using System.Globalization;
using Octo.Services.Soulseek;

namespace Octo.Services.Listening;

/// <summary>
/// Turns Subsonic scrobble and OpenSubsonic playback-report calls for Octo's
/// temporary ids into one de-duplicated listening session.
/// </summary>
public sealed class ExternalPlaybackScrobbler
{
    private static readonly TimeSpan SessionLifetime = TimeSpan.FromHours(6);
    private readonly ExternalIdRegistry _idRegistry;
    private readonly IListeningSubmissionQueue _submissions;
    private readonly ILogger<ExternalPlaybackScrobbler> _logger;
    private readonly TimeProvider _clock;
    private readonly object _gate = new();
    private readonly Dictionary<string, PlaybackSession> _sessions = new();

    public ExternalPlaybackScrobbler(
        ExternalIdRegistry idRegistry,
        IListeningSubmissionQueue submissions,
        ILogger<ExternalPlaybackScrobbler> logger,
        TimeProvider? clock = null)
    {
        _idRegistry = idRegistry;
        _submissions = submissions;
        _logger = logger;
        _clock = clock ?? TimeProvider.System;
    }

    public void ReportScrobble(IReadOnlyDictionary<string, string> parameters)
    {
        var mediaId = Value(parameters, "id");
        if (!TryBuildTrack(mediaId, out var track)) return;

        var now = _clock.GetUtcNow();
        var submission = ParseBoolean(Value(parameters, "submission"), defaultValue: true);
        var explicitStart = ParseSubsonicTime(Value(parameters, "time"));
        var sessionKey = SessionKey(parameters, mediaId);
        ListeningSubmission? queued = null;

        lock (_gate)
        {
            RemoveExpiredSessions(now);
            _sessions.TryGetValue(sessionKey, out var session);

            // A new now-playing notification after a completed session is a replay
            // of the same song, not a duplicate of the old session.
            if (session is null
                || (!submission && session.Scrobbled)
                || (explicitStart.HasValue
                    && Math.Abs((session.StartedAt - explicitStart.Value).TotalSeconds) > 5))
            {
                session = NewSession(track, explicitStart ?? now, now);
                _sessions[sessionKey] = session;
            }
            else
            {
                session.Track = track; // later calls often contain richer routing data
                session.LastSeenAt = now;
            }

            if (!submission)
            {
                if (!session.NowPlayingSent)
                {
                    session.NowPlayingSent = true;
                    queued = ToSubmission(session, parameters);
                }
            }
            else if (!session.Scrobbled && IsEligibleDuration(track.DurationSeconds))
            {
                // The Subsonic client owns the threshold when it sends a submission.
                // When no start notification preceded it, estimate the required
                // Last.fm/ListenBrainz start timestamp from the known duration.
                if (explicitStart.HasValue) session.StartedAt = explicitStart.Value;
                else if (!session.NowPlayingSent && track.DurationSeconds is > 0)
                    session.StartedAt = now - TimeSpan.FromSeconds(track.DurationSeconds.Value);
                session.Scrobbled = true;
                queued = ToSubmission(session, parameters);
            }
        }

        if (queued is null) return;
        if (submission) _submissions.QueueScrobble(queued);
        else _submissions.QueueNowPlaying(queued);
    }

    public void ReportPlayback(IReadOnlyDictionary<string, string> parameters)
    {
        if (!string.IsNullOrWhiteSpace(Value(parameters, "mediaType"))
            && !Value(parameters, "mediaType").Equals("song", StringComparison.OrdinalIgnoreCase))
        {
            return;
        }

        var mediaId = Value(parameters, "mediaId");
        if (string.IsNullOrWhiteSpace(mediaId)) mediaId = Value(parameters, "id");
        if (!TryBuildTrack(mediaId, out var track)) return;

        var state = Value(parameters, "state").Trim().ToLowerInvariant();
        if (state is not ("starting" or "playing" or "paused" or "stopped")) return;

        var positionMs = long.TryParse(Value(parameters, "positionMs"),
                NumberStyles.Integer, CultureInfo.InvariantCulture, out var parsedPosition)
            ? Math.Clamp(parsedPosition, 0, 86_400_000L)
            : 0;
        var playbackRate = double.TryParse(Value(parameters, "playbackRate"),
                NumberStyles.Float, CultureInfo.InvariantCulture, out var parsedRate)
            && parsedRate is > 0 and <= 16
                ? parsedRate
                : 1d;
        var ignoreScrobble = ParseBoolean(Value(parameters, "ignoreScrobble"), false);
        var now = _clock.GetUtcNow();
        var sessionKey = SessionKey(parameters, mediaId);
        ListeningSubmission? nowPlaying = null;
        ListeningSubmission? scrobble = null;

        lock (_gate)
        {
            RemoveExpiredSessions(now);
            _sessions.TryGetValue(sessionKey, out var session);
            if (session is null || state == "starting")
            {
                var start = now - TimeSpan.FromMilliseconds(positionMs);
                session = NewSession(track, start, now);
                _sessions[sessionKey] = session;
            }
            else
            {
                AccumulatePlayedTime(session, now);
                session.Track = track;
                session.LastSeenAt = now;
            }

            session.PositionMs = positionMs;
            session.ProgressUpdatedAt = now;
            session.LastState = state;
            session.PlaybackRate = playbackRate;
            if ((state == "starting" || state == "playing") && !session.NowPlayingSent)
            {
                session.NowPlayingSent = true;
                nowPlaying = ToSubmission(session, parameters);
            }

            if (!ignoreScrobble
                && !session.Scrobbled
                && HasReachedThreshold(track.DurationSeconds, session.PlayedMs))
            {
                session.Scrobbled = true;
                scrobble = ToSubmission(session, parameters);
            }
        }

        if (nowPlaying is not null) _submissions.QueueNowPlaying(nowPlaying);
        if (scrobble is not null) _submissions.QueueScrobble(scrobble);
    }

    internal static bool HasReachedThreshold(int? durationSeconds, double playedMs)
    {
        if (!IsEligibleDuration(durationSeconds)) return false;
        var thresholdMs = durationSeconds is > 0
            ? Math.Min(durationSeconds.Value * 500L, 240_000L)
            : 240_000L;
        return playedMs >= thresholdMs;
    }

    private bool TryBuildTrack(string mediaId, out ListeningTrack track)
    {
        var routing = _idRegistry.Lookup(mediaId)
            ?? SoulseekMetadataService.TryDecodeExternalId(mediaId);
        if (routing is null
            || string.IsNullOrWhiteSpace(routing.Artist)
            || string.IsNullOrWhiteSpace(routing.Title))
        {
            _logger.LogWarning(
                "Cannot scrobble temporary id {Id}: its artist/title routing is no longer available",
                mediaId);
            track = new ListeningTrack();
            return false;
        }

        var youtubeId = routing.YouTubeId?.Trim();
        track = new ListeningTrack
        {
            MediaId = mediaId,
            Artist = routing.Artist.Trim(),
            Title = routing.Title.Trim(),
            Album = NonBlank(routing.Release?.Title ?? routing.Album),
            AlbumArtist = NonBlank(routing.Release?.Artist),
            DurationSeconds = routing.Duration is > 0 ? routing.Duration : null,
            TrackNumber = routing.Track is > 0 ? routing.Track : null,
            Isrc = NonBlank(routing.Isrc),
            MusicService = string.IsNullOrWhiteSpace(youtubeId) ? null : "youtube.com",
            OriginUrl = string.IsNullOrWhiteSpace(youtubeId)
                ? null
                : "https://www.youtube.com/watch?v=" + Uri.EscapeDataString(youtubeId),
        };
        return true;
    }

    private static PlaybackSession NewSession(
        ListeningTrack track,
        DateTimeOffset startedAt,
        DateTimeOffset now) => new()
    {
        Track = track,
        StartedAt = startedAt,
        LastSeenAt = now,
        ProgressUpdatedAt = now,
    };

    private static ListeningSubmission ToSubmission(
        PlaybackSession session,
        IReadOnlyDictionary<string, string> parameters) => new()
    {
        Track = session.Track,
        StartedAt = session.StartedAt,
        MediaPlayer = NonBlank(Value(parameters, "c")),
    };

    private void RemoveExpiredSessions(DateTimeOffset now)
    {
        foreach (var key in _sessions
                     .Where(pair => now - pair.Value.LastSeenAt > SessionLifetime)
                     .Select(pair => pair.Key)
                     .ToList())
        {
            _sessions.Remove(key);
        }
    }

    private static void AccumulatePlayedTime(PlaybackSession session, DateTimeOffset now)
    {
        if (session.LastState != "playing") return;
        var elapsedMs = Math.Max(0, (now - session.ProgressUpdatedAt).TotalMilliseconds);
        var playedMs = elapsedMs * session.PlaybackRate;
        if (session.Track.DurationSeconds is > 0)
        {
            session.PlayedMs = Math.Min(
                session.Track.DurationSeconds.Value * 1000d,
                session.PlayedMs + playedMs);
        }
        else
        {
            session.PlayedMs += playedMs;
        }
    }

    private static string SessionKey(
        IReadOnlyDictionary<string, string> parameters,
        string mediaId) => string.Join('\u001f',
        Value(parameters, "u").Trim().ToLowerInvariant(),
        Value(parameters, "c").Trim().ToLowerInvariant(),
        mediaId);

    private static DateTimeOffset? ParseSubsonicTime(string value)
    {
        if (!long.TryParse(value, NumberStyles.Integer, CultureInfo.InvariantCulture, out var timestamp)
            || timestamp <= 0)
        {
            return null;
        }
        try
        {
            // The Subsonic API specifies milliseconds. Accept seconds too because
            // older clients in the wild used Last.fm-style epoch seconds here.
            return timestamp < 100_000_000_000L
                ? DateTimeOffset.FromUnixTimeSeconds(timestamp)
                : DateTimeOffset.FromUnixTimeMilliseconds(timestamp);
        }
        catch (ArgumentOutOfRangeException)
        {
            return null;
        }
    }

    private static bool ParseBoolean(string value, bool defaultValue)
    {
        if (bool.TryParse(value, out var parsed)) return parsed;
        if (value == "1") return true;
        if (value == "0") return false;
        return defaultValue;
    }

    private static bool IsEligibleDuration(int? durationSeconds)
        => durationSeconds is null or > 30;

    private static string? NonBlank(string? value)
        => string.IsNullOrWhiteSpace(value) ? null : value.Trim();

    private static string Value(IReadOnlyDictionary<string, string> parameters, string key)
        => parameters.TryGetValue(key, out var value) ? value ?? string.Empty : string.Empty;

    private sealed class PlaybackSession
    {
        public ListeningTrack Track { get; set; } = new();
        public DateTimeOffset StartedAt { get; set; }
        public DateTimeOffset LastSeenAt { get; set; }
        public DateTimeOffset ProgressUpdatedAt { get; set; }
        public long PositionMs { get; set; }
        public double PlayedMs { get; set; }
        public double PlaybackRate { get; set; } = 1d;
        public string LastState { get; set; } = string.Empty;
        public bool NowPlayingSent { get; set; }
        public bool Scrobbled { get; set; }
    }
}
