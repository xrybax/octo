namespace Octo.Models.Domain;

/// <summary>The selected release, shared by every track of an album download.
/// Recording searches must not replace these fields with a different edition.</summary>
public sealed record AlbumReleaseMetadata(
    string DeezerId, string? ArtistDeezerId, string Title, string Artist,
    int? Year, string? Genre, string? CoverUrl, string? Label, int TotalTracks)
{
    public void ApplyTo(Song song)
    {
        song.Album = Title;
        song.AlbumArtist = Artist;
        song.Year = Year;
        song.Genre = Genre;
        song.CoverArtUrl = CoverUrl;
        song.CoverArtUrlLarge = CoverUrl;
        song.Label = Label;
        song.TotalTracks = TotalTracks;
    }
}
