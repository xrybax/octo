using Microsoft.Extensions.Logging.Abstractions;
using Moq;
using Octo.Services.CoverArt;
using Octo.Services.Soulseek;

namespace Octo.Tests;

public sealed class CoverArtAggregatorTests
{
    [Theory]
    [InlineData(RoutingKind.Artist)]
    [InlineData(RoutingKind.Album)]
    public async Task NamesakesWithDifferentProviderIds_DoNotShareCachedImages(RoutingKind kind)
    {
        var source = new Mock<ICoverArtSource>();
        source.SetupGet(s => s.Name).Returns("deezer");
        source.Setup(s => s.TryFetchAsync(It.IsAny<SoulseekRouting>(), It.IsAny<CancellationToken>()))
            .ReturnsAsync((SoulseekRouting routing, CancellationToken _) =>
                new byte[] { routing.ExternalArtistId == "42" ? (byte)42 : (byte)7 });
        using var covers = new CoverArtAggregator(new[] { source.Object }, NullLogger<CoverArtAggregator>.Instance);
        var first = new SoulseekRouting
        {
            Kind = kind, Artist = "Feel", Album = "Feel", ExternalArtistId = "42", ExternalAlbumId = "99",
        };
        var second = new SoulseekRouting
        {
            Kind = kind, Artist = "Feel", Album = "Feel", ExternalArtistId = "7", ExternalAlbumId = "77",
        };

        Assert.Equal(new byte[] { 42 }, await covers.GetCoverAsync(first));
        Assert.Equal(new byte[] { 7 }, await covers.GetCoverAsync(second));
        Assert.Equal(new byte[] { 42 }, await covers.GetCoverAsync(first));
        source.Verify(s => s.TryFetchAsync(It.IsAny<SoulseekRouting>(), It.IsAny<CancellationToken>()), Times.Exactly(2));
    }

    [Fact]
    public async Task IdentifiedArtist_DoesNotFallBackToUnrelatedNameOnlyPhoto()
    {
        var deezer = new Mock<ICoverArtSource>();
        deezer.SetupGet(s => s.Name).Returns("deezer");
        deezer.Setup(s => s.TryFetchAsync(It.IsAny<SoulseekRouting>(), It.IsAny<CancellationToken>()))
            .ReturnsAsync((byte[]?)null);
        var fallback = new Mock<ICoverArtSource>();
        fallback.SetupGet(s => s.Name).Returns("lastfm");
        fallback.Setup(s => s.TryFetchAsync(It.IsAny<SoulseekRouting>(), It.IsAny<CancellationToken>()))
            .ReturnsAsync(new byte[] { 7 });
        using var covers = new CoverArtAggregator(new[] { deezer.Object, fallback.Object },
            NullLogger<CoverArtAggregator>.Instance);

        var bytes = await covers.GetCoverAsync(new SoulseekRouting
        {
            Kind = RoutingKind.Artist, Artist = "Feel", ExternalArtistId = "42",
        });

        Assert.Null(bytes);
        fallback.Verify(s => s.TryFetchAsync(It.IsAny<SoulseekRouting>(), It.IsAny<CancellationToken>()), Times.Never);
    }
}
