using Microsoft.Extensions.Logging.Abstractions;
using Octo.Models.Domain;
using Octo.Services.Listening;
using Octo.Services.Soulseek;

namespace Octo.Tests;

public sealed class ExternalPlaybackScrobblerTests
{
    private sealed class RecordingQueue : IListeningSubmissionQueue
    {
        public List<ListeningSubmission> NowPlaying { get; } = new();
        public List<ListeningSubmission> Scrobbles { get; } = new();
        public void QueueNowPlaying(ListeningSubmission submission) => NowPlaying.Add(submission);
        public void QueueScrobble(ListeningSubmission submission) => Scrobbles.Add(submission);
    }

    private sealed class ManualTimeProvider(DateTimeOffset now) : TimeProvider
    {
        public DateTimeOffset Now { get; set; } = now;
        public override DateTimeOffset GetUtcNow() => Now;
    }

    [Fact]
    public void PlaybackReportSendsNowPlayingAndScrobblesAtHalfTheTrack()
    {
        var (service, queue, id, clock) = Build(durationSeconds: 200);

        service.ReportPlayback(Playback(id, "starting", 0));
        service.ReportPlayback(Playback(id, "playing", 0));
        clock.Now = clock.Now.AddMilliseconds(99_999);
        service.ReportPlayback(Playback(id, "playing", 99_999));
        Assert.Single(queue.NowPlaying);
        Assert.Empty(queue.Scrobbles);

        clock.Now = clock.Now.AddMilliseconds(1);
        service.ReportPlayback(Playback(id, "playing", 100_000));

        var listen = Assert.Single(queue.Scrobbles);
        Assert.Equal("Radiohead", listen.Track.Artist);
        Assert.Equal("Nude", listen.Track.Title);
        Assert.Equal("In Rainbows", listen.Track.Album);
        Assert.Equal("Radiohead", listen.Track.AlbumArtist);
        Assert.Equal(3, listen.Track.TrackNumber);
        Assert.Equal("GBSTK0700001", listen.Track.Isrc);
        Assert.Equal("youtube.com", listen.Track.MusicService);
        Assert.Equal("https://www.youtube.com/watch?v=video-id", listen.Track.OriginUrl);
        Assert.Equal("Feishin", listen.MediaPlayer);
    }

    [Fact]
    public void TenMinuteTrackUsesFourMinuteThreshold()
    {
        var (service, queue, id, clock) = Build(durationSeconds: 600);

        service.ReportPlayback(Playback(id, "starting", 0));
        service.ReportPlayback(Playback(id, "playing", 0));
        clock.Now = clock.Now.AddMilliseconds(239_999);
        service.ReportPlayback(Playback(id, "playing", 239_999));
        Assert.Empty(queue.Scrobbles);

        clock.Now = clock.Now.AddMilliseconds(1);
        service.ReportPlayback(Playback(id, "playing", 240_000));
        Assert.Single(queue.Scrobbles);
    }

    [Fact]
    public void IgnoreScrobbleStillAllowsNowPlayingButNeverCompletesListen()
    {
        var (service, queue, id, clock) = Build(durationSeconds: 120);
        var report = Playback(id, "playing", 0);
        report["ignoreScrobble"] = "true";

        service.ReportPlayback(report);
        clock.Now = clock.Now.AddSeconds(120);
        report["positionMs"] = "120000";
        report["state"] = "stopped";
        service.ReportPlayback(report);

        Assert.Single(queue.NowPlaying);
        Assert.Empty(queue.Scrobbles);
    }

    [Fact]
    public void SeekingPastTheThresholdDoesNotCountAsListening()
    {
        var (service, queue, id, clock) = Build(durationSeconds: 200);
        service.ReportPlayback(Playback(id, "starting", 0));
        service.ReportPlayback(Playback(id, "playing", 0));

        clock.Now = clock.Now.AddSeconds(1);
        service.ReportPlayback(Playback(id, "playing", 150_000));

        Assert.Empty(queue.Scrobbles);
    }

    [Fact]
    public void PlaybackReportAndClassicScrobbleAreDeduplicated()
    {
        var (service, queue, id, clock) = Build(durationSeconds: 120);
        service.ReportPlayback(Playback(id, "starting", 0));
        service.ReportPlayback(Playback(id, "playing", 0));
        clock.Now = clock.Now.AddSeconds(60);
        service.ReportPlayback(Playback(id, "playing", 60_000));

        service.ReportScrobble(new Dictionary<string, string>
        {
            ["id"] = id,
            ["submission"] = "true",
            ["u"] = "mortifer",
            ["c"] = "Feishin",
        });

        Assert.Single(queue.Scrobbles);
    }

    [Fact]
    public void NewStartingStateAllowsImmediateReplayOfTheSameSong()
    {
        var (service, queue, id, clock) = Build(durationSeconds: 100);
        service.ReportPlayback(Playback(id, "starting", 0));
        service.ReportPlayback(Playback(id, "playing", 0));
        clock.Now = clock.Now.AddSeconds(50);
        service.ReportPlayback(Playback(id, "playing", 50_000));

        clock.Now = clock.Now.AddMinutes(2);
        service.ReportPlayback(Playback(id, "starting", 0));
        service.ReportPlayback(Playback(id, "playing", 0));
        clock.Now = clock.Now.AddSeconds(50);
        service.ReportPlayback(Playback(id, "playing", 50_000));

        Assert.Equal(2, queue.NowPlaying.Count);
        Assert.Equal(2, queue.Scrobbles.Count);
    }

    [Fact]
    public void ClassicSubmissionUsesProvidedMillisecondTimestamp()
    {
        var (service, queue, id, _) = Build(durationSeconds: 180);
        var startedAt = new DateTimeOffset(2026, 9, 13, 3, 4, 5, TimeSpan.Zero);

        service.ReportScrobble(new Dictionary<string, string>
        {
            ["id"] = id,
            ["submission"] = "true",
            ["time"] = startedAt.ToUnixTimeMilliseconds().ToString(),
            ["u"] = "mortifer",
            ["c"] = "Tempo",
        });

        Assert.Equal(startedAt, Assert.Single(queue.Scrobbles).StartedAt);
    }

    [Fact]
    public void TracksOfThirtySecondsOrLessAreNotScrobbled()
    {
        var (service, queue, id, _) = Build(durationSeconds: 30);

        service.ReportPlayback(Playback(id, "playing", 30_000));
        service.ReportScrobble(new Dictionary<string, string>
        {
            ["id"] = id,
            ["submission"] = "true",
            ["u"] = "mortifer",
            ["c"] = "Feishin",
        });

        Assert.Empty(queue.Scrobbles);
    }

    private static (ExternalPlaybackScrobbler Service, RecordingQueue Queue, string Id, ManualTimeProvider Clock)
        Build(int durationSeconds)
    {
        var registry = new ExternalIdRegistry();
        var id = registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Song,
            Artist = "Radiohead",
            Title = "Nude",
            Album = "In Rainbows",
            Duration = durationSeconds,
            Track = 3,
            Isrc = "GBSTK0700001",
            YouTubeId = "video-id",
            Release = new AlbumReleaseMetadata(
                "14880659", "399", "In Rainbows", "Radiohead", 2007,
                "Alternative", null, "XL", 10),
        });
        var queue = new RecordingQueue();
        var clock = new ManualTimeProvider(
            new DateTimeOffset(2026, 9, 13, 12, 0, 0, TimeSpan.Zero));
        var service = new ExternalPlaybackScrobbler(
            registry, queue, NullLogger<ExternalPlaybackScrobbler>.Instance, clock);
        return (service, queue, id, clock);
    }

    private static Dictionary<string, string> Playback(string id, string state, long positionMs) => new()
    {
        ["mediaId"] = id,
        ["mediaType"] = "song",
        ["positionMs"] = positionMs.ToString(),
        ["state"] = state,
        ["u"] = "mortifer",
        ["c"] = "Feishin",
    };
}
