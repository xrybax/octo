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
