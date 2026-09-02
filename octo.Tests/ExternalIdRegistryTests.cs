using Octo.Services.Soulseek;

namespace Octo.Tests;

public class ExternalIdRegistryTests
{
    private readonly ExternalIdRegistry _registry;

    public ExternalIdRegistryTests()
    {
        _registry = new ExternalIdRegistry();
    }

    [Fact]
    public void Register_SameRouting_ProducesSameId()
    {
        // Arrange
        var a = new SoulseekRouting { Kind = RoutingKind.Song, Artist = "Radiohead", Title = "Nude", Duration = 255 };
        var b = new SoulseekRouting { Kind = RoutingKind.Song, Artist = "Radiohead", Title = "Nude", Duration = 255 };

        // Act
        var idA = _registry.Register(a);
        var idB = _registry.Register(b);

        // Assert
        Assert.Equal(idA, idB);
    }

    [Fact]
    public void Register_DifferentKindsSameNames_ProduceDifferentIds()
    {
        // The Kind prefix keeps a song id distinct from its album and artist ids,
        // otherwise getCoverArt would return the wrong scope's artwork.
        var songId = _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Song, Artist = "Radiohead", Title = "In Rainbows"
        });
        var albumId = _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Album, Artist = "Radiohead", Album = "In Rainbows"
        });
        var artistId = _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Artist, Artist = "Radiohead"
        });

        Assert.NotEqual(songId, albumId);
        Assert.NotEqual(albumId, artistId);
        Assert.NotEqual(songId, artistId);
    }

    [Fact]
    public void Register_AlbumWithoutDeezerId_DoesNotClobberKnownExternalAlbumId()
    {
        // Arrange: an album search registers the precise Deezer id.
        var fromSearch = new SoulseekRouting
        {
            Kind = RoutingKind.Album,
            Artist = "Radiohead",
            Album = "In Rainbows",
            ExternalAlbumId = "14880659",
        };
        var id = _registry.Register(fromSearch);

        // Act: a song row later mints the same artist+album with no Deezer id. It hashes
        // to the same key, so a naive overwrite would drop the id we already resolved.
        var fromSongRow = new SoulseekRouting
        {
            Kind = RoutingKind.Album,
            Artist = "Radiohead",
            Album = "In Rainbows",
        };
        var sameId = _registry.Register(fromSongRow);

        // Assert
        Assert.Equal(id, sameId);
        Assert.Equal("14880659", _registry.Lookup(id)!.ExternalAlbumId);
    }

    [Fact]
    public void Register_AlbumWithDeezerId_OverwritesAnEarlierUnknownId()
    {
        // The preserve must only fill blanks, never block a real update.
        var id = _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Album, Artist = "Radiohead", Album = "In Rainbows"
        });
        Assert.Null(_registry.Lookup(id)!.ExternalAlbumId);

        _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Album,
            Artist = "Radiohead",
            Album = "In Rainbows",
            ExternalAlbumId = "14880659",
        });

        Assert.Equal("14880659", _registry.Lookup(id)!.ExternalAlbumId);
    }

    [Fact]
    public void Register_WeakerRouting_PreservesArtistIdAndReleaseType()
    {
        var id = _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Album,
            Artist = "Radiohead",
            Album = "In Rainbows",
            ExternalAlbumId = "14880659",
            ExternalArtistId = "399",
            ReleaseType = "album",
            CoverArtUrl = "https://cdn/in-rainbows.jpg",
        });

        var sameId = _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Album,
            Artist = "Radiohead",
            Album = "In Rainbows",
        });

        Assert.Equal(id, sameId);
        var routing = _registry.Lookup(id)!;
        Assert.Equal("14880659", routing.ExternalAlbumId);
        Assert.Equal("399", routing.ExternalArtistId);
        Assert.Equal("album", routing.ReleaseType);
        Assert.Equal("https://cdn/in-rainbows.jpg", routing.CoverArtUrl);
    }

    [Fact]
    public void Register_ArtistWithoutDeezerId_DoesNotClobberKnownId()
    {
        var id = _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Artist,
            Artist = "Radiohead",
            ExternalArtistId = "399",
        });

        _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Artist,
            Artist = "Radiohead",
        });

        Assert.Equal("399", _registry.Lookup(id)!.ExternalArtistId);
    }

    [Fact]
    public void Lookup_UnknownId_ReturnsNull()
    {
        Assert.Null(_registry.Lookup("nonexistent"));
    }
}
