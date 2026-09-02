using System.Net;
using System.Text;
using System.Text.Json;
using System.Xml.Linq;
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

public sealed class ArtistDiscographyIntegrationTests
{
    private const string LocalJson =
        """
        {"subsonic-response":{"status":"ok","version":"1.16.1","artist":{"id":"local-id","name":"Opał","albumCount":1,"album":[{"id":"local-album","name":"Pierwszy","artist":"Opał","artistId":"local-id","songCount":10}]}}}
        """;

    private const string LocalXml =
        """
        <subsonic-response xmlns="http://subsonic.org/restapi" status="ok" version="1.16.1">
          <artist id="local-id" name="Opał" albumCount="1">
            <album id="local-album" name="Pierwszy" artist="Opał" artistId="local-id" songCount="10" />
          </artist>
        </subsonic-response>
        """;

    [Fact]
    public async Task GetArtistJson_MergesMissingReleasesAndKeepsLocalParent()
    {
        var metadata = DiscographyMetadata();
        await using var factory = CreateFactory(metadata, LocalJson, "application/json");

        using var client = factory.CreateClient();
        using var response = await client.GetAsync("/rest/getArtist?id=local-id&f=json");

        response.EnsureSuccessStatusCode();
        using var doc = JsonDocument.Parse(await response.Content.ReadAsStringAsync());
        var artist = doc.RootElement.GetProperty("subsonic-response").GetProperty("artist");
        var albums = artist.GetProperty("album");
        Assert.Equal(2, artist.GetProperty("albumCount").GetInt32());
        Assert.Equal(2, albums.GetArrayLength());
        Assert.Equal("local-album", albums[0].GetProperty("id").GetString());
        Assert.False(albums[0].GetProperty("isExternal").GetBoolean());
        Assert.Equal("Missing EP", albums[1].GetProperty("name").GetString());
        Assert.Equal("local-id", albums[1].GetProperty("artistId").GetString());
        Assert.Equal("ep", albums[1].GetProperty("releaseTypes")[0].GetString());

        metadata.Verify(m => m.SearchArtistsAsync("Opał", 5), Times.Once);
        metadata.Verify(m => m.GetArtistAlbumsAsync("soulseek", "external-artist"), Times.Once);
    }

    [Fact]
    public async Task GetArtistXml_MergesMissingReleasesAndUpdatesCount()
    {
        var metadata = DiscographyMetadata();
        await using var factory = CreateFactory(metadata, LocalXml, "application/xml");

        using var client = factory.CreateClient();
        using var response = await client.GetAsync("/rest/getArtist?id=local-id&f=xml");

        response.EnsureSuccessStatusCode();
        var doc = XDocument.Parse(await response.Content.ReadAsStringAsync());
        var ns = doc.Root!.GetDefaultNamespace();
        var artist = doc.Root.Element(ns + "artist")!;
        var albums = artist.Elements(ns + "album").ToList();
        Assert.Equal("2", artist.Attribute("albumCount")?.Value);
        Assert.Equal(2, albums.Count);
        Assert.Equal("Missing EP", albums[1].Attribute("name")?.Value);
        Assert.Equal("local-id", albums[1].Attribute("artistId")?.Value);
    }

    [Fact]
    public async Task GetArtist_WhenMetadataFails_ReturnsNavidromeBodyUnchanged()
    {
        var metadata = new Mock<IMusicMetadataService>();
        metadata.Setup(m => m.SearchArtistsAsync("Opał", 5))
            .ThrowsAsync(new HttpRequestException("catalog unavailable"));
        await using var factory = CreateFactory(metadata, LocalJson, "application/json");

        using var client = factory.CreateClient();
        using var response = await client.GetAsync("/rest/getArtist?id=local-id&f=json");

        response.EnsureSuccessStatusCode();
        Assert.Equal(LocalJson, await response.Content.ReadAsStringAsync());
    }

    private static Mock<IMusicMetadataService> DiscographyMetadata()
    {
        var metadata = new Mock<IMusicMetadataService>();
        metadata.Setup(m => m.SearchArtistsAsync("Opał", 5))
            .ReturnsAsync(new List<Artist>
            {
                new()
                {
                    Id = "wrong-artist",
                    Name = "Opałowski",
                    ExternalProvider = "soulseek",
                    ExternalId = "wrong-artist",
                },
                new()
                {
                    Id = "external-artist",
                    Name = "OPAŁ",
                    ExternalProvider = "soulseek",
                    ExternalId = "external-artist",
                },
            });
        metadata.Setup(m => m.GetArtistAlbumsAsync("soulseek", "external-artist"))
            .ReturnsAsync(new List<Album>
            {
                // Normalized duplicate of the local album; must not be appended.
                new()
                {
                    Id = "external-duplicate",
                    Title = "PIERWSZY",
                    Artist = "External Name",
                    ArtistId = "external-artist",
                    ReleaseTypes = new List<string> { "album" },
                },
                new()
                {
                    Id = "external-ep",
                    Title = "Missing EP",
                    Artist = "External Name",
                    ArtistId = "external-artist",
                    Year = 2026,
                    SongCount = 4,
                    ReleaseTypes = new List<string> { "ep" },
                    ExternalProvider = "soulseek",
                    ExternalId = "external-ep",
                },
            });
        return metadata;
    }

    private static WebApplicationFactory<Program> CreateFactory(
        Mock<IMusicMetadataService> metadata, string navidromeBody, string mediaType)
    {
        var library = new Mock<ILocalLibraryService>();
        library.Setup(l => l.ParseSongId("local-id"))
            .Returns((false, null, null));
        var downloads = new Mock<IDownloadService>();
        var httpFactory = new StaticHttpClientFactory(navidromeBody, mediaType);

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

    private sealed class StaticHttpClientFactory : IHttpClientFactory
    {
        private readonly HttpMessageHandler _handler;

        public StaticHttpClientFactory(string body, string mediaType)
        {
            _handler = new StaticResponseHandler(body, mediaType);
        }

        public HttpClient CreateClient(string name) => new(_handler, disposeHandler: false);
    }

    private sealed class StaticResponseHandler(string body, string mediaType) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request,
            CancellationToken cancellationToken)
        {
            var response = new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new StringContent(body, Encoding.UTF8, mediaType),
            };
            return Task.FromResult(response);
        }
    }
}
