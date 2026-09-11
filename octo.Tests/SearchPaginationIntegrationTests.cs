using System.Net;
using System.Text;
using System.Text.Json;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Mvc.Testing;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.DependencyInjection.Extensions;
using Microsoft.Extensions.Hosting;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Options;
using Moq;
using Octo.Models.Domain;
using Octo.Models.Settings;
using Octo.Services;
using Octo.Services.Common;
using Octo.Services.LastFm;
using Octo.Services.Local;

namespace Octo.Tests;

public sealed class SearchPaginationIntegrationTests
{
    [Fact]
    public async Task Search3_ContinuationReturnsTheNextExternalTwentyWithoutRepeatingPageOne()
    {
        var lastFmHandler = new LastFmTrackSearchHandler();
        var lastFm = new LastFmService(
            new HttpClient(lastFmHandler),
            Options.Create(new LastFmSettings { ApiKey = "test-key" }),
            Options.Create(new MetadataSettings()),
            new Mock<ILogger<LastFmService>>().Object);

        var metadata = new Mock<IMusicMetadataService>();
        metadata.Setup(service => service.SearchSongsByArtistTitleAsync(
                It.IsAny<string>(), It.IsAny<string>(), 1, It.IsAny<int?>()))
            .ReturnsAsync((string artist, string title, int _, int? _) =>
                new List<Song>
                {
                    new()
                    {
                        Id = $"external-{title.Split(' ')[^1]}",
                        Artist = artist,
                        Title = title,
                        Duration = 180,
                        IsLocal = false,
                        ExternalProvider = "soulseek",
                    },
                });
        metadata.Setup(service => service.EnrichExternalSongsAsync(
                It.IsAny<List<Song>>(), It.IsAny<CancellationToken>()))
            .Returns(Task.CompletedTask);
        metadata.Setup(service => service.EnrichExternalSearchPageAsync(
                It.IsAny<List<Song>>(), It.IsAny<CancellationToken>()))
            .Returns(Task.CompletedTask);
        metadata.Setup(service => service.PrewarmYouTubeIdsAsync(
                It.IsAny<IEnumerable<Song>>(), 12, It.IsAny<CancellationToken>()))
            .Returns(Task.CompletedTask);

        using var externalSearch = new ExternalSearchService(
            metadata.Object,
            new Mock<ILogger<ExternalSearchService>>().Object,
            lastFm);
        await using var factory = CreateFactory(metadata, externalSearch);
        using var client = factory.CreateClient();

        var first = await SearchSongIds(client, offset: 0);
        var second = await SearchSongIds(client, offset: 20);

        Assert.Equal(20, first.Count);
        Assert.Equal(Enumerable.Range(1, 12).Select(i => $"local-{i}"), first.Take(12));
        Assert.Equal(Enumerable.Range(1, 8).Select(i => $"external-{i}"), first.Skip(12));
        Assert.Equal(Enumerable.Range(9, 20).Select(i => $"external-{i}"), second);
        Assert.Empty(first.Intersect(second));
        Assert.Equal(1, lastFmHandler.TrackSearchCalls);
        metadata.Verify(service => service.EnrichExternalSearchPageAsync(
            It.Is<List<Song>>(songs => songs.Count == 20),
            It.IsAny<CancellationToken>()), Times.Once);
    }

    private static async Task<List<string>> SearchSongIds(HttpClient client, int offset)
    {
        using var response = await client.GetAsync(
            $"/rest/search3?query=example&songCount=20&songOffset={offset}&albumCount=0&artistCount=0&f=json&u=test&c=test");
        response.EnsureSuccessStatusCode();
        using var doc = JsonDocument.Parse(await response.Content.ReadAsStringAsync());
        return doc.RootElement
            .GetProperty("subsonic-response")
            .GetProperty("searchResult3")
            .GetProperty("song")
            .EnumerateArray()
            .Select(song => song.GetProperty("id").GetString()!)
            .ToList();
    }

    private static WebApplicationFactory<Program> CreateFactory(
        Mock<IMusicMetadataService> metadata,
        ExternalSearchService externalSearch)
    {
        var library = new Mock<ILocalLibraryService>();
        var downloads = new Mock<IDownloadService>();
        var httpFactory = new LocalSearchHttpClientFactory();

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
                    services.RemoveAll<ExternalSearchService>();
                    services.AddSingleton<IHttpClientFactory>(httpFactory);
                    services.AddSingleton(metadata.Object);
                    services.AddSingleton(library.Object);
                    services.AddSingleton(downloads.Object);
                    services.AddSingleton(externalSearch);
                });
            });
    }

    private sealed class LocalSearchHttpClientFactory : IHttpClientFactory
    {
        public HttpClient CreateClient(string name) => new(new LocalSearchHandler());
    }

    private sealed class LocalSearchHandler : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken)
        {
            var songs = Enumerable.Range(1, 12)
                .Select(i => new
                {
                    id = $"local-{i}",
                    title = $"Local {i}",
                    artist = "Local Artist",
                    album = "Local Album",
                    duration = 180,
                });
            var body = JsonSerializer.Serialize(new
            {
                subsonic_response = new
                {
                    status = "ok",
                    version = "1.16.1",
                    searchResult3 = new { song = songs, album = Array.Empty<object>(), artist = Array.Empty<object>() },
                },
            }).Replace("subsonic_response", "subsonic-response", StringComparison.Ordinal);

            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new StringContent(body, Encoding.UTF8, "application/json"),
            });
        }
    }

    private sealed class LastFmTrackSearchHandler : HttpMessageHandler
    {
        public int TrackSearchCalls { get; private set; }

        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken)
        {
            TrackSearchCalls++;
            var tracks = Enumerable.Range(1, 50)
                .Select(i => new { name = $"Track {i}", artist = $"Artist {i}" });
            var body = JsonSerializer.Serialize(new
            {
                results = new { trackmatches = new { track = tracks } },
            });
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new StringContent(body, Encoding.UTF8, "application/json"),
            });
        }
    }
}
