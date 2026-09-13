namespace Octo.Services.Listening;

public sealed class ListeningTrack
{
    public string MediaId { get; set; } = string.Empty;
    public string Artist { get; set; } = string.Empty;
    public string Title { get; set; } = string.Empty;
    public string? Album { get; set; }
    public string? AlbumArtist { get; set; }
    public int? DurationSeconds { get; set; }
    public int? TrackNumber { get; set; }
    public string? Isrc { get; set; }
    public string? MusicService { get; set; }
    public string? OriginUrl { get; set; }
}

public sealed class ListeningSubmission
{
    public ListeningTrack Track { get; set; } = new();
    public DateTimeOffset StartedAt { get; set; }
    public string? MediaPlayer { get; set; }
}

public sealed class PendingScrobble
{
    public string Id { get; set; } = Guid.NewGuid().ToString("N");
    public ListeningSubmission Submission { get; set; } = new();
    public List<string> PendingSinks { get; set; } = new();
    public DateTimeOffset CreatedAt { get; set; } = DateTimeOffset.UtcNow;
    public DateTimeOffset NextAttemptAt { get; set; } = DateTimeOffset.UtcNow;
    public int Attempts { get; set; }
    public string? LastError { get; set; }
}
