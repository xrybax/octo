//! Chromaprint fingerprints via fpcalc.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tracing::warn;

use super::net_format;
use super::tool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FingerprintOutcome {
    /// fpcalc produced a fingerprint.
    Ok,

    /// fpcalc is not in the image, timed out, or failed in a way that says nothing about
    /// the FILE. Verification must treat this as "no opinion" and keep the download.
    Unavailable,

    /// fpcalc started, read the file, and could not decode audio out of it. That is a fact
    /// about the file: a zero-filled or truncated FLAC that TagLib still reports a duration
    /// for decodes to nothing here.
    Undecodable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintResult {
    pub outcome: FingerprintOutcome,
    pub fingerprint: Option<String>,
    pub decoded_seconds: i32,
}

impl FingerprintResult {
    pub fn new(outcome: FingerprintOutcome, fingerprint: Option<String>, decoded_seconds: i32) -> Self {
        Self {
            outcome,
            fingerprint,
            decoded_seconds,
        }
    }

    fn missing() -> Self {
        Self::new(FingerprintOutcome::Unavailable, None, 0)
    }
}

/// A chromaprint as chromaprint's own numbers: one 32-bit item for every 1365 samples at
/// 11025 Hz, about eight a second.
#[derive(Debug, Clone, PartialEq)]
pub struct RawFingerprint {
    pub items: Vec<u32>,
    /// The whole file's length as fpcalc read it, however much of it was fingerprinted.
    pub seconds: f64,
}

/// Chromaprint fingerprints, via the fpcalc binary.
///
/// Modelled on LastFmRadioAudioTranscoder's ffmpeg shell-out, with one deliberate
/// difference: that one translates a missing binary into an error, because radio
/// cannot work without ffmpeg. Here a missing binary must NEVER fail a download that has
/// already succeeded. Every failure path is "no opinion", and the caller keeps the file.
/// Numbers each resampled copy's temporary file, so two at once never share one.
static SPEED_RUN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Debug, Default)]
pub struct AudioFingerprinter {
    /// Latched so a misbuilt image costs one log line, not a spawned process per download.
    binary_missing: AtomicBool,
}

impl AudioFingerprinter {
    pub fn new() -> Self {
        Self::default()
    }

    /// How much audio to fingerprint and how long fpcalc may take both come from settings:
    /// the first trades decode time against nothing much, and the second depends entirely on
    /// how slow the user's storage is.
    pub async fn fingerprint(
        &self,
        path: &Path,
        length_seconds: i32,
        timeout_seconds: i32,
    ) -> FingerprintResult {
        let Some(run) = self.run_fpcalc(&[], path, length_seconds, timeout_seconds).await else {
            return FingerprintResult::missing();
        };

        let stdout = String::from_utf8_lossy(&run.stdout);
        if let Some(parsed) = Self::parse_fpcalc_json(Some(&stdout)) {
            return parsed;
        }

        // Exit 0 with nothing parseable is a shape we do not understand: no opinion.
        if run.exit_code == 0 {
            warn!("fpcalc produced no usable JSON for {}", path.display());
            return FingerprintResult::missing();
        }

        // Started, read the file, refused it. The file is the problem. This is the branch a
        // zero-filled FLAC lands in when its header still claims a runtime.
        warn!(
            "fpcalc could not decode {} (exit {}): {}",
            path.display(),
            run.exit_code,
            run.stderr.trim()
        );
        FingerprintResult::new(FingerprintOutcome::Undecodable, None, 0)
    }

    /// The first `length_seconds` of the file as chromaprint's own numbers (`fpcalc -raw`), the
    /// form two fingerprints are lined up and compared in. None for every failure: fpcalc
    /// missing or slow, or a file it could not read.
    pub async fn raw_fingerprint(
        &self,
        path: &Path,
        length_seconds: i32,
        timeout_seconds: i32,
    ) -> Option<RawFingerprint> {
        let run = self
            .run_fpcalc(&["-raw"], path, length_seconds, timeout_seconds)
            .await?;
        let parsed = Self::parse_fpcalc_raw_json(Some(&String::from_utf8_lossy(&run.stdout)));
        if parsed.is_none() {
            warn!(
                "fpcalc gave no raw fingerprint for {} (exit {}): {}",
                path.display(),
                run.exit_code,
                run.stderr.trim()
            );
        }
        parsed
    }

    /// fpcalc -json over `length_seconds` of the file, with `extra` arguments in front. None when
    /// fpcalc is missing (latched), fails to run or does not finish within the timeout.
    async fn run_fpcalc(
        &self,
        extra: &[&str],
        path: &Path,
        length_seconds: i32,
        timeout_seconds: i32,
    ) -> Option<tool::ToolRun> {
        if self.binary_missing.load(Ordering::Relaxed) {
            return None;
        }

        let length = length_seconds.to_string();
        let path_text = path.to_string_lossy();
        let mut arguments = extra.to_vec();
        arguments.extend(["-json", "-length", &length, &path_text]);
        let mut child = match tool::spawn("fpcalc", &arguments) {
            Ok(child) => child,
            Err(error) => {
                self.binary_missing.store(true, Ordering::Relaxed);
                warn!(
                    "fpcalc is not in this image, so download verification can only ever accept: {error}. \
                     Rebuild with libchromaprint-tools installed."
                );
                return None;
            }
        };

        // Deliberately not linked to any caller token. Verification runs between a finished
        // transfer and the file joining the library, and a caller who has already left must
        // not be able to wave an unchecked file through. (A negative timeout times out at
        // once.)
        let timeout = Duration::from_secs(timeout_seconds.max(0).unsigned_abs() as u64);
        match tokio::time::timeout(timeout, tool::collect(&mut child, true)).await {
            Ok(Ok(run)) => Some(run),
            Ok(Err(error)) => {
                tool::kill(&mut child);
                warn!("fpcalc did not finish for {}: {error}", path.display());
                None
            }
            Err(_) => {
                tool::kill(&mut child);
                warn!(
                    "fpcalc did not finish for {}: The operation was canceled.",
                    path.display()
                );
                None
            }
        }
    }

    /// fpcalc -raw -json writes {"duration": 253.14, "fingerprint": [2741973374, ...]}. None
    /// unless that is the shape, there is at least one number and every number fits 32 bits.
    pub(crate) fn parse_fpcalc_raw_json(stdout: Option<&str>) -> Option<RawFingerprint> {
        let root: serde_json::Value = serde_json::from_str(stdout?.trim()).ok()?;
        let items = root
            .get("fingerprint")?
            .as_array()?
            .iter()
            .map(|item| item.as_u64().and_then(|n| u32::try_from(n).ok()))
            .collect::<Option<Vec<u32>>>()
            .filter(|items| !items.is_empty())?;
        let seconds = root
            .get("duration")
            .and_then(serde_json::Value::as_f64)
            .filter(|s| s.is_finite() && *s > 0.0)?;
        Some(RawFingerprint { items, seconds })
    }

    /// The fingerprint of the file played back at `speed` times its speed, pitch and all, as a
    /// record player at the wrong speed would: a sped-up or slowed copy played back at the
    /// inverse of its change fingerprints as the recording it was made from. ffmpeg writes the
    /// first `length_seconds` of that to a temporary WAV, which fpcalc reads and which is
    /// removed afterwards. Every failure is "no opinion".
    pub async fn fingerprint_at_speed(
        &self,
        path: &Path,
        speed: f64,
        length_seconds: i32,
        timeout_seconds: i32,
    ) -> FingerprintResult {
        if !(0.5..=2.0).contains(&speed) || self.binary_missing.load(Ordering::Relaxed) {
            return FingerprintResult::missing();
        }
        let wav = std::env::temp_dir().join(format!(
            "octo-speed-{}-{}.wav",
            std::process::id(),
            SPEED_RUN.fetch_add(1, Ordering::Relaxed)
        ));
        let filter = format!("aresample=44100,asetrate={:.0},aresample=44100", 44_100.0 * speed);
        let length = length_seconds.max(1).to_string();
        let source = path.to_string_lossy();
        let target = wav.to_string_lossy();
        let arguments = [
            "-v", "error", "-nostdin", "-y", "-i", &source, "-vn", "-af", &filter, "-t", &length, "-f",
            "wav", &target,
        ];
        let timeout = Duration::from_secs(timeout_seconds.max(0).unsigned_abs() as u64);
        let made = match tool::spawn("ffmpeg", &arguments) {
            Ok(mut child) => match tokio::time::timeout(timeout, tool::collect(&mut child, false)).await {
                Ok(Ok(run)) => run.exit_code == 0,
                Ok(Err(_)) | Err(_) => {
                    tool::kill(&mut child);
                    false
                }
            },
            Err(error) => {
                warn!("ffmpeg could not start for a resampled fingerprint: {error}");
                false
            }
        };
        let result = if made {
            self.fingerprint(&wav, length_seconds, timeout_seconds).await
        } else {
            FingerprintResult::missing()
        };
        let _ = std::fs::remove_file(&wav);
        result
    }

    /// fpcalc -json writes one object to stdout: {"duration": 253.14, "fingerprint": "AQADt..."}.
    /// Simpler than the transcoder's stderr scraping, which is why -json is used.
    ///
    /// Returns None when the output is not fpcalc's shape at all, so the caller can tell
    /// "I do not understand this" apart from "this file has no audio".
    pub(crate) fn parse_fpcalc_json(stdout: Option<&str>) -> Option<FingerprintResult> {
        let stdout = stdout?;
        if stdout.trim().is_empty() {
            return None;
        }
        let root: serde_json::Value = serde_json::from_str(stdout).ok()?;
        let root = root.as_object()?;

        // A "fingerprint" that is not a string is "not fpcalc's shape" like any other.
        let fingerprint = match root.get("fingerprint") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(text)) => Some(text.clone()),
            Some(_) => return None,
        };
        let fingerprint = fingerprint.filter(|text| !text.trim().is_empty())?;

        let seconds = match root.get("duration") {
            Some(serde_json::Value::Number(number)) => number
                .as_f64()
                .map(|value| net_format::round(value) as i32)
                .unwrap_or(0),
            _ => 0,
        };

        // A fingerprint over zero seconds of audio is not a fingerprint of anything.
        if seconds <= 0 {
            return Some(FingerprintResult::new(FingerprintOutcome::Undecodable, None, 0));
        }

        Some(FingerprintResult::new(
            FingerprintOutcome::Ok,
            Some(fingerprint),
            seconds,
        ))
    }
}

#[cfg(test)]
mod tests {
    //! fpcalc's output is the only thing standing between a finished download and an AcoustID
    //! lookup, and every failure here has to mean "no opinion" rather than "reject", because a
    //! rejection deletes a file and writes a deny-list entry.

    use super::*;

    #[test]
    fn parse_fpcalc_json_real_output_reads_fingerprint_and_duration() {
        let result = AudioFingerprinter::parse_fpcalc_json(Some(
            r#"{"duration": 253.14, "fingerprint": "AQADtEmkRUkSHR2CH1-OHT-Ko8dxHT2OH0eP4-jxHT2O"}"#,
        ))
        .expect("fpcalc's shape");

        assert_eq!(result.outcome, FingerprintOutcome::Ok);
        assert_eq!(result.decoded_seconds, 253);
        assert!(
            result
                .fingerprint
                .as_deref()
                .is_some_and(|fp| fp.starts_with("AQADtEmkRUkSHR2CH1"))
        );
    }

    /// A fingerprint over zero seconds of audio is not a fingerprint of anything. This is the
    /// shape a zero-filled FLAC produces when its header still claims a runtime.
    #[test]
    fn parse_fpcalc_json_zero_duration_is_undecodable_not_ok() {
        let result =
            AudioFingerprinter::parse_fpcalc_json(Some(r#"{"duration": 0, "fingerprint": "AQADtEmk"}"#))
                .expect("fpcalc's shape");

        assert_eq!(result.outcome, FingerprintOutcome::Undecodable);
    }

    /// None means "this is not fpcalc's shape at all", which the caller must tell apart from
    /// "this file has no audio": one keeps the file, the other deletes it.
    #[test]
    fn parse_fpcalc_json_unrecognized_output_returns_null_rather_than_throwing() {
        let cases = [
            "",
            "   ",
            "not json at all",
            "[1,2,3]",
            r#"{"duration": 253.14}"#,
            r#"{"duration": 253.14, "fingerprint": ""}"#,
        ];
        for stdout in cases {
            assert_eq!(
                AudioFingerprinter::parse_fpcalc_json(Some(stdout)),
                None,
                "stdout {stdout:?}"
            );
        }
        assert_eq!(AudioFingerprinter::parse_fpcalc_json(None), None);
    }

    #[test]
    fn parse_fpcalc_raw_json_reads_the_numbers_and_the_length() {
        let raw = AudioFingerprinter::parse_fpcalc_raw_json(Some(
            r#"{"duration": 294.99, "fingerprint": [2741973374, 3782160638, 0]}"#,
        ))
        .expect("fpcalc's raw shape");
        assert_eq!(raw.items, vec![2_741_973_374, 3_782_160_638, 0]);
        assert_eq!(raw.seconds, 294.99);
    }

    /// Anything else is no fingerprint: a compare must never run on half-read numbers.
    #[test]
    fn parse_fpcalc_raw_json_refuses_any_other_shape() {
        let cases = [
            "",
            "not json",
            r#"{"duration": 12.0, "fingerprint": "AQADtEmk"}"#,
            r#"{"duration": 12.0, "fingerprint": []}"#,
            r#"{"duration": 12.0, "fingerprint": [1, -2]}"#,
            r#"{"duration": 12.0, "fingerprint": [4294967296]}"#,
            r#"{"duration": 0, "fingerprint": [1, 2]}"#,
            r#"{"fingerprint": [1, 2]}"#,
        ];
        for stdout in cases {
            assert_eq!(
                AudioFingerprinter::parse_fpcalc_raw_json(Some(stdout)),
                None,
                "stdout {stdout:?}"
            );
        }
    }

    /// Real fpcalc gives about eight items a second, less the first two seconds or so its filters
    /// need. Skips cleanly where fpcalc is not installed.
    #[tokio::test]
    async fn fpcalc_gives_a_raw_fingerprint_of_a_tone() {
        if !which("fpcalc") {
            eprintln!("skipped: fpcalc is not on the PATH");
            return;
        }
        crate::audio::test_support::require_ffmpeg!();
        let dir = tempfile::tempdir().expect("a temp dir");
        crate::audio::test_support::run_ffmpeg(
            dir.path(),
            "-f lavfi -i anoisesrc=color=pink:duration=12:sample_rate=44100:amplitude=0.3 -c:a flac noise.flac",
        );
        let raw = AudioFingerprinter::new()
            .raw_fingerprint(&dir.path().join("noise.flac"), 120, 30)
            .await
            .expect("a raw fingerprint");
        assert!((60..=100).contains(&raw.items.len()), "{} items", raw.items.len());
        assert!((raw.seconds - 12.0).abs() < 0.5, "{}", raw.seconds);
    }

    /// The contract under test is "verification never fails a download", not which flavor of
    /// failure a given machine produces: there may be no fpcalc on the machine and there is no
    /// audio file at this path on any of them.
    #[tokio::test]
    async fn fingerprint_async_missing_file_or_binary_never_throws_into_the_download_loop() {
        let fingerprinter = AudioFingerprinter::new();
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("octo-no-such-file.flac");

        let result = fingerprinter.fingerprint(&path, 120, 5).await;

        assert_ne!(result.outcome, FingerprintOutcome::Ok);
    }

    // ---- The missing-tool latch and a real fpcalc ----------------------------------------

    /// A missing fpcalc is latched: the second call does not try to start it again. Skips when
    /// fpcalc is installed, since then nothing is latched.
    #[tokio::test]
    async fn a_missing_fpcalc_is_latched() {
        if which("fpcalc") {
            eprintln!("skipped: fpcalc is on the PATH, so there is nothing to latch");
            return;
        }
        let fingerprinter = AudioFingerprinter::new();
        let path = std::env::temp_dir().join("octo-no-such-file.flac");
        assert_eq!(
            fingerprinter.fingerprint(&path, 120, 5).await.outcome,
            FingerprintOutcome::Unavailable
        );
        assert!(fingerprinter.binary_missing.load(Ordering::Relaxed));
        assert_eq!(
            fingerprinter.fingerprint(&path, 120, 5).await.outcome,
            FingerprintOutcome::Unavailable
        );
    }

    /// Real fpcalc on a real tone. Skips cleanly where fpcalc is not installed.
    #[tokio::test]
    async fn fpcalc_fingerprints_a_tone() {
        if !which("fpcalc") {
            eprintln!("skipped: fpcalc is not on the PATH");
            return;
        }
        crate::audio::test_support::require_ffmpeg!();
        let dir = tempfile::tempdir().expect("a temp dir");
        crate::audio::test_support::run_ffmpeg(
            dir.path(),
            "-f lavfi -i anoisesrc=color=pink:duration=12:sample_rate=44100:amplitude=0.3 -c:a flac noise.flac",
        );
        let result = AudioFingerprinter::new()
            .fingerprint(&dir.path().join("noise.flac"), 120, 30)
            .await;
        assert_eq!(result.outcome, FingerprintOutcome::Ok, "{result:?}");
        assert_eq!(result.decoded_seconds, 12);
    }

    fn which(program: &str) -> bool {
        std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
    }
}
