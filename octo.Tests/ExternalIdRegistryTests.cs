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
    public void Register_NameOnlyAlbum_DoesNotAliasProviderBackedAlbum()
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

        // Act: a legacy song row later mints the same artist+album with no Deezer id.
        var fromSongRow = new SoulseekRouting
        {
            Kind = RoutingKind.Album,
            Artist = "Radiohead",
            Album = "In Rainbows",
        };
        var weakId = _registry.Register(fromSongRow);

        // Assert
        Assert.NotEqual(id, weakId);
        Assert.Equal("14880659", _registry.Lookup(id)!.ExternalAlbumId);
        Assert.Null(_registry.Lookup(weakId)!.ExternalAlbumId);
    }

    [Fact]
    public void Register_ProviderBackedAlbum_DoesNotOverwriteLegacyNameOnlyRoute()
    {
        var weakId = _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Album, Artist = "Radiohead", Album = "In Rainbows"
        });
        Assert.Null(_registry.Lookup(weakId)!.ExternalAlbumId);

        var strongId = _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Album,
            Artist = "Radiohead",
            Album = "In Rainbows",
            ExternalAlbumId = "14880659",
        });

        Assert.NotEqual(weakId, strongId);
        Assert.Null(_registry.Lookup(weakId)!.ExternalAlbumId);
        Assert.Equal("14880659", _registry.Lookup(strongId)!.ExternalAlbumId);
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
            Artist = "RADIOHEAD",
            Album = "Alternate display title",
            ExternalAlbumId = "14880659",
        });

        Assert.Equal(id, sameId);
        var routing = _registry.Lookup(id)!;
        Assert.Equal("14880659", routing.ExternalAlbumId);
        Assert.Equal("399", routing.ExternalArtistId);
        Assert.Equal("album", routing.ReleaseType);
        Assert.Equal("https://cdn/in-rainbows.jpg", routing.CoverArtUrl);
    }

    [Fact]
    public void Register_SameProviderArtistId_PreservesKnownMetadata()
    {
        var id = _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Artist,
            Artist = "Radiohead",
            ExternalArtistId = "399",
            CoverArtUrl = "https://cdn/radiohead.jpg",
        });

        var sameId = _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Artist,
            Artist = "RADIOHEAD",
            ExternalArtistId = "399",
        });

        Assert.Equal(id, sameId);
        Assert.Equal("399", _registry.Lookup(id)!.ExternalArtistId);
        Assert.Equal("https://cdn/radiohead.jpg", _registry.Lookup(id)!.CoverArtUrl);
    }

    [Fact]
    public void Register_SameArtistNameWithDifferentProviderIds_ProducesDifferentIds()
    {
        var first = _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Artist,
            Artist = "Feel",
            ExternalArtistId = "111",
        });
        var second = _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Artist,
            Artist = "Feel",
            ExternalArtistId = "222",
        });

        Assert.NotEqual(first, second);
        Assert.Equal("111", _registry.Lookup(first)!.ExternalArtistId);
        Assert.Equal("222", _registry.Lookup(second)!.ExternalArtistId);
    }

    [Fact]
    public void Register_SameAlbumNameWithDifferentProviderIds_ProducesDifferentIds()
    {
        var first = _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Album,
            Artist = "Feel",
            Album = "Feel",
            ExternalAlbumId = "333",
        });
        var second = _registry.Register(new SoulseekRouting
        {
            Kind = RoutingKind.Album,
            Artist = "Feel",
            Album = "Feel",
            ExternalAlbumId = "444",
        });

        Assert.NotEqual(first, second);
    }

    [Fact]
    public void Lookup_UnknownId_ReturnsNull()
    {
        Assert.Null(_registry.Lookup("nonexistent"));
    }
}
