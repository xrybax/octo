using System.Net;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Options;
using Moq;
using Moq.Protected;
using Octo.Models.Settings;
using Octo.Services.CoverArt;
using Octo.Services.Soulseek;

namespace Octo.Tests;

public sealed class DeezerCoverArtLookupTests
{
    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public async Task ArtistCover_FollowsProviderIdOrSourceRecordingInsteadOfNameSearch(bool knownId)
    {
        var requested = new List<Uri>();
        var handler = new Mock<HttpMessageHandler>();
        handler.Protected()
            .Setup<Task<HttpResponseMessage>>("SendAsync",
                ItExpr.IsAny<HttpRequestMessage>(), ItExpr.IsAny<CancellationToken>())
            .ReturnsAsync((HttpRequestMessage request, CancellationToken _) =>
            {
                var uri = request.RequestUri!;
                requested.Add(uri);
                HttpContent content = uri.Host == "cdn.example"
                    ? new ByteArrayContent(new byte[] { 42 })
                    : new StringContent(knownId
                        ? """{"id":42,"name":"Feel","picture_xl":"https://cdn.example/correct.jpg"}"""
                        : """{"data":[{"title":"Recording","artist":{"id":42,"name":"Feel","picture_xl":"https://cdn.example/correct.jpg"}}]}""");
                return new HttpResponseMessage(HttpStatusCode.OK) { Content = content };
            });
        var factory = new Mock<IHttpClientFactory>();
        factory.Setup(f => f.CreateClient(It.IsAny<string>())).Returns(new HttpClient(handler.Object));
        var lookup = new DeezerCoverArtLookup(factory.Object, Options.Create(new MetadataSettings()),
            new Mock<ILogger<DeezerCoverArtLookup>>().Object);

        var bytes = await lookup.TryFetchAsync(new SoulseekRouting
        {
            Kind = RoutingKind.Artist, Artist = "Feel", Title = "Recording",
            ExternalArtistId = knownId ? "42" : null,
        });

        Assert.Equal(new byte[] { 42 }, bytes);
        Assert.Equal(knownId ? "/artist/42" : "/search", requested[0].AbsolutePath);
        if (!knownId)
            Assert.Contains("track:\"Recording\"", Uri.UnescapeDataString(requested[0].Query));
        Assert.Equal("https://cdn.example/correct.jpg", requested[1].ToString());
    }

    [Fact]
    public async Task DirectRoutingUrl_SkipsCatalogSearch()
    {
        var requested = new List<string>();
        var handler = new Mock<HttpMessageHandler>();
        handler.Protected()
            .Setup<Task<HttpResponseMessage>>("SendAsync",
                ItExpr.IsAny<HttpRequestMessage>(),
                ItExpr.IsAny<CancellationToken>())
            .ReturnsAsync((HttpRequestMessage request, CancellationToken _) =>
            {
                requested.Add(request.RequestUri!.ToString());
                return new HttpResponseMessage(HttpStatusCode.OK)
                {
                    Content = new ByteArrayContent(new byte[] { 1, 2, 3 }),
                };
            });
        var factory = new Mock<IHttpClientFactory>();
        factory.Setup(f => f.CreateClient(It.IsAny<string>()))
            .Returns(new HttpClient(handler.Object));
        var lookup = new DeezerCoverArtLookup(
            factory.Object,
            Options.Create(new MetadataSettings()),
            new Mock<ILogger<DeezerCoverArtLookup>>().Object);
        var routing = new SoulseekRouting
        {
            Kind = RoutingKind.Album,
            Artist = "Radiohead",
            Album = "In Rainbows",
            CoverArtUrl = "https://cdn.example/direct.jpg",
        };

        var bytes = await lookup.TryFetchAsync(routing);

        Assert.Equal(new byte[] { 1, 2, 3 }, bytes);
        Assert.Equal(new[] { "https://cdn.example/direct.jpg" }, requested);
    }
}
