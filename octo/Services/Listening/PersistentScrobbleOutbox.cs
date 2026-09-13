using System.Text.Json;

namespace Octo.Services.Listening;

/// <summary>
/// Small durable outbox for completed listens. Metadata is captured while the
/// temporary Octo id still exists, then retained across network failures and
/// container restarts until every configured destination accepts it.
/// </summary>
public sealed class PersistentScrobbleOutbox
{
    private const int MaxEntries = 5_000;
    private readonly string _path;
    private readonly ILogger<PersistentScrobbleOutbox> _logger;
    private readonly object _gate = new();
    private List<PendingScrobble>? _entries;

    public PersistentScrobbleOutbox(
        string path,
        ILogger<PersistentScrobbleOutbox> logger)
    {
        _path = path;
        _logger = logger;
    }

    public void Add(
        ListeningSubmission submission,
        IReadOnlyCollection<string> targetSinks,
        DateTimeOffset now)
    {
        if (targetSinks.Count == 0) return;

        lock (_gate)
        {
            var entries = LoadLocked();
            entries.Add(new PendingScrobble
            {
                Submission = Clone(submission),
                PendingSinks = targetSinks.Distinct(StringComparer.OrdinalIgnoreCase).ToList(),
                CreatedAt = now,
                NextAttemptAt = now,
            });
            if (entries.Count > MaxEntries)
            {
                var dropCount = entries.Count - MaxEntries;
                entries.RemoveRange(0, dropCount);
                _logger.LogWarning(
                    "Scrobble outbox reached {Limit} entries; dropped {Count} oldest listens",
                    MaxEntries, dropCount);
            }
            SaveLocked(entries);
        }
    }

    public IReadOnlyList<PendingScrobble> GetDue(DateTimeOffset now, int limit = 100)
    {
        lock (_gate)
        {
            return LoadLocked()
                .Where(entry => entry.PendingSinks.Count > 0 && entry.NextAttemptAt <= now)
                .OrderBy(entry => entry.CreatedAt)
                .Take(Math.Max(0, limit))
                .Select(Clone)
                .ToList();
        }
    }

    public void MarkDelivered(string id, string sinkName)
    {
        lock (_gate)
        {
            var entries = LoadLocked();
            var entry = entries.FirstOrDefault(candidate => candidate.Id == id);
            if (entry is null) return;

            entry.PendingSinks.RemoveAll(sink =>
                sink.Equals(sinkName, StringComparison.OrdinalIgnoreCase));
            if (entry.PendingSinks.Count == 0) entries.Remove(entry);
            SaveLocked(entries);
        }
    }

    public void Defer(
        string id,
        string error,
        DateTimeOffset nextAttemptAt)
    {
        lock (_gate)
        {
            var entries = LoadLocked();
            var entry = entries.FirstOrDefault(candidate => candidate.Id == id);
            if (entry is null) return;

            entry.Attempts++;
            entry.LastError = error;
            entry.NextAttemptAt = nextAttemptAt;
            SaveLocked(entries);
        }
    }

    internal int Count
    {
        get
        {
            lock (_gate) return LoadLocked().Count;
        }
    }

    private List<PendingScrobble> LoadLocked()
    {
        if (_entries is not null) return _entries;
        try
        {
            if (!File.Exists(_path)) return _entries = new List<PendingScrobble>();
            var json = File.ReadAllText(_path);
            _entries = string.IsNullOrWhiteSpace(json)
                ? new List<PendingScrobble>()
                : JsonSerializer.Deserialize<List<PendingScrobble>>(json)
                    ?? new List<PendingScrobble>();
            _entries.RemoveAll(entry => entry.Submission?.Track is null
                || string.IsNullOrWhiteSpace(entry.Submission.Track.Artist)
                || string.IsNullOrWhiteSpace(entry.Submission.Track.Title)
                || entry.PendingSinks is null
                || entry.PendingSinks.Count == 0);
            return _entries;
        }
        catch (Exception ex)
        {
            _logger.LogWarning(
                "Scrobble outbox load failed ({Message}); starting with an empty queue",
                ex.Message);
            return _entries = new List<PendingScrobble>();
        }
    }

    private void SaveLocked(List<PendingScrobble> entries)
    {
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(_path) ?? ".");
            var temporaryPath = _path + ".tmp";
            File.WriteAllText(temporaryPath, JsonSerializer.Serialize(entries));
            File.Move(temporaryPath, _path, overwrite: true);
        }
        catch (Exception ex)
        {
            // The in-memory copy remains queued even if persistence is temporarily
            // unavailable. Scrobbling must never interfere with audio playback.
            _logger.LogWarning("Scrobble outbox save failed: {Message}", ex.Message);
        }
    }

    private static PendingScrobble Clone(PendingScrobble source) => new()
    {
        Id = source.Id,
        Submission = Clone(source.Submission),
        PendingSinks = source.PendingSinks.ToList(),
        CreatedAt = source.CreatedAt,
        NextAttemptAt = source.NextAttemptAt,
        Attempts = source.Attempts,
        LastError = source.LastError,
    };

    private static ListeningSubmission Clone(ListeningSubmission source) => new()
    {
        StartedAt = source.StartedAt,
        MediaPlayer = source.MediaPlayer,
        Track = new ListeningTrack
        {
            MediaId = source.Track.MediaId,
            Artist = source.Track.Artist,
            Title = source.Track.Title,
            Album = source.Track.Album,
            AlbumArtist = source.Track.AlbumArtist,
            DurationSeconds = source.Track.DurationSeconds,
            TrackNumber = source.Track.TrackNumber,
            Isrc = source.Track.Isrc,
            MusicService = source.Track.MusicService,
            OriginUrl = source.Track.OriginUrl,
        },
    };
}
