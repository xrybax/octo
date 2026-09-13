using Microsoft.Extensions.Logging.Abstractions;
using Octo.Services.Listening;

namespace Octo.Tests;

public sealed class PersistentScrobbleOutboxTests
{
    [Fact]
    public void PendingDestinationsSurviveReloadAndSuccessfulOnesAreRemovedIndividually()
    {
        var directory = Path.Combine(Path.GetTempPath(), $"octo-scrobble-{Guid.NewGuid():N}");
        var path = Path.Combine(directory, "outbox.json");
        try
        {
            var now = new DateTimeOffset(2026, 9, 13, 12, 0, 0, TimeSpan.Zero);
            var store = NewStore(path);
            store.Add(Submission(), ["lastfm", "listenbrainz"], now);
            Assert.Equal(1, store.Count);

            var reloaded = NewStore(path);
            var pending = Assert.Single(reloaded.GetDue(now));
            Assert.Equal("Radiohead", pending.Submission.Track.Artist);
            Assert.Equal(2, pending.PendingSinks.Count);

            reloaded.MarkDelivered(pending.Id, "lastfm");
            var remaining = Assert.Single(NewStore(path).GetDue(now));
            Assert.Equal(["listenbrainz"], remaining.PendingSinks);

            reloaded.MarkDelivered(pending.Id, "listenbrainz");
            Assert.Equal(0, NewStore(path).Count);
        }
        finally
        {
            if (Directory.Exists(directory)) Directory.Delete(directory, recursive: true);
        }
    }

    [Fact]
    public void DeferredListenIsHiddenUntilItsRetryTime()
    {
        var directory = Path.Combine(Path.GetTempPath(), $"octo-scrobble-{Guid.NewGuid():N}");
        var path = Path.Combine(directory, "outbox.json");
        try
        {
            var now = new DateTimeOffset(2026, 9, 13, 12, 0, 0, TimeSpan.Zero);
            var store = NewStore(path);
            store.Add(Submission(), ["lastfm"], now);
            var pending = Assert.Single(store.GetDue(now));

            store.Defer(pending.Id, "offline", now.AddMinutes(5));

            Assert.Empty(store.GetDue(now.AddMinutes(4)));
            var due = Assert.Single(store.GetDue(now.AddMinutes(5)));
            Assert.Equal(1, due.Attempts);
            Assert.Equal("offline", due.LastError);
        }
        finally
        {
            if (Directory.Exists(directory)) Directory.Delete(directory, recursive: true);
        }
    }

    private static PersistentScrobbleOutbox NewStore(string path) =>
        new(path, NullLogger<PersistentScrobbleOutbox>.Instance);

    private static ListeningSubmission Submission() => new()
    {
        StartedAt = new DateTimeOffset(2026, 9, 13, 11, 56, 40, TimeSpan.Zero),
        MediaPlayer = "Feishin",
        Track = new ListeningTrack
        {
            MediaId = "temporary-id",
            Artist = "Radiohead",
            Title = "Nude",
            DurationSeconds = 200,
        },
    };
}
