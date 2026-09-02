namespace Octo.Models.Domain;

/// <summary>
/// Represents an album
/// </summary>
public class Album
{
    public string Id { get; set; } = string.Empty;
    public string Title { get; set; } = string.Empty;
    public string Artist { get; set; } = string.Empty;
    public string? ArtistId { get; set; }
    public int? Year { get; set; }
    public int? SongCount { get; set; }
    public string? CoverArtUrl { get; set; }
    public string? Genre { get; set; }
    /// <summary>
    /// OpenSubsonic release categories such as album, ep or single. Keeping this as a
    /// collection lets modern clients group a complete artist discography without
    /// guessing from the track count or title.
    /// </summary>
    public List<string> ReleaseTypes { get; set; } = new();
    public bool IsLocal { get; set; }
    public string? ExternalProvider { get; set; }
    public string? ExternalId { get; set; }
    public List<Song> Songs { get; set; } = new();
}
