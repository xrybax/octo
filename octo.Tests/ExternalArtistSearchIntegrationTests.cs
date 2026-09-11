using System.Net;
using System.Text;
using System.Text.Json;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Mvc.Testing;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.DependencyInjection.Extensions;
using Microsoft.Extensions.Hosting;
using Moq;
using Octo.Models.Domain;
using Octo.Services;
using Octo.Services.Local;

namespace Octo.Tests;

public sealed class ExternalArtistSearchIntegrationTests
{
    private const string EmptyLocalSearch =
        """
        {"subsonic-response":{"status":"ok","version":"1.16.1","searchResult3":{"artist":[],"album":[],"song":[]}}}
        """;

    [Fact]
    public async Task Search3_IncludesExternalArtistsRequestedByTheClient()
    {
        var metadata = new Mock<IMusicMetadataService>();
        metadata.Setup(service => service.SearchArtistsAsync("Feel", 20))
            .ReturnsAsync(new List<Artist>
            {
                new()
                {
                    Id = "external-feel",
                    Name = "Feel",
                    AlbumCount = 6,
                    ImageUrl = "https://cdn/feel.jpg",
                    IsLocal = false,
                    ExternalProvider = "soulseek",
                    ExternalId = "external-feel",
                },
            });

        await using var factory = CreateFactory(metadata);
        using var client = factory.CreateClient();
        using var response = await client.GetAsync(
            "/rest/search3?query=Feel&songCount=0&albumCount=0&artistCount=20&f=json");

        response.EnsureSuccessStatusCode();
        using var doc = JsonDocument.Parse(await response.Content.ReadAsStringAsync());
        var artists = doc.RootElement
            .GetProperty("subsonic-response")
            .GetProperty("searchResult3")
            .GetProperty("artist");

        var artist = Assert.Single(artists.EnumerateArray());
        Assert.Equal("external-feel", artist.GetProperty("id").GetString());
        Assert.Equal("Feel", artist.GetProperty("name").GetString());
        metadata.Verify(service => service.SearchArtistsAsync("Feel", 20), Times.Once);
    }

    private static WebApplicationFactory<Program> CreateFactory(
        Mock<IMusicMetadataService> metadata)
    {
        var library = new Mock<ILocalLibraryService>();
        var downloads = new Mock<IDownloadService>();
        var httpFactory = new StaticHttpClientFactory(EmptyLocalSearch);

        return new WebApplicationFactory<Program>()
            .WithWebHostBuilder(builder =>
            {
                builder.ConfigureAppConfiguration((_, config) =>
                    config.AddInMemoryCollection(new Dictionary<string, string?>
                    {
                        ["Subsonic:Url"] = "http://navidrome.test",
                        ["Library:DownloadPath"] = Path.GetTempPath(),
                    }));
                builder.ConfigureServices(services =>
                {
                    services.RemoveAll<IHostedService>();
                    services.RemoveAll<IHttpClientFactory>();
                    services.RemoveAll<IMusicMetadataService>();
                    services.RemoveAll<ILocalLibraryService>();
                    services.RemoveAll<IDownloadService>();
                    services.AddSingleton<IHttpClientFactory>(httpFactory);
                    services.AddSingleton(metadata.Object);
                    services.AddSingleton(library.Object);
                    services.AddSingleton(downloads.Object);
                });
            });
    }

    private sealed class StaticHttpClientFactory(string body) : IHttpClientFactory
    {
        public HttpClient CreateClient(string name) => new(new StaticHandler(body));
    }

    private sealed class StaticHandler(string body) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken)
        {
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new StringContent(body, Encoding.UTF8, "application/json"),
            });
        }
    }
}
