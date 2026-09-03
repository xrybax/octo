using System.Net;
using System.Text;
using System.Text.Json;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Options;
using Moq;
using Octo.Models.Domain;
using Octo.Models.Settings;
using Octo.Services;
using Octo.Services.Common;
using Octo.Services.LastFm;

namespace Octo.Tests;

public sealed class ExternalSearchServiceTests
{
    [Fact]
    public async Task HealthyTrackSearchSkipsTopTracksAndMovesYouTubeOffCriticalPath()
    {
        var lastFmStarted = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var handler = new LastFmSearchHandler(
            trackCount: 50,
            trackSearchStarted: () => lastFmStarted.TrySetResult());
        var lastFm = new LastFmService(
            new HttpClient(handler),
            Options.Create(new LastFmSettings { ApiKey = "test-key" }),
            Options.Create(new MetadataSettings()),
            new Mock<ILogger<LastFmService>>().Object);

        var metadata = new Mock<IMusicMetadataService>();
        var prefetchOverlappedLastFm = false;
        metadata.Setup(m => m.PrefetchSearchMetadataAsync(
                "nergal", 25, It.IsAny<CancellationToken>()))
            .Returns(async (string query, int limit, CancellationToken token) =>
            {
                await lastFmStarted.Task.WaitAsync(TimeSpan.FromSeconds(1));
                prefetchOverlappedLastFm = true;
                return 25;
            });
        metadata.Setup(m => m.SearchSongsByArtistTitleAsync(
                It.IsAny<string>(), It.IsAny<string>(), 1, It.IsAny<int?>()))
            .ReturnsAsync((string artist, string title, int _, int? _) =>
                new List<Song>
                {
                    new()
                    {
                        Id = $"{artist}|{title}",
                        Artist = artist,
                        Title = title,
                        Duration = 180,
                        IsLocal = false,
                    },
                });
        metadata.Setup(m => m.EnrichExternalSongsAsync(
                It.IsAny<List<Song>>(), It.IsAny<CancellationToken>()))
            .Returns(Task.CompletedTask);
        metadata.Setup(m => m.PrewarmYouTubeIdsAsync(
                It.IsAny<IEnumerable<Song>>(), 12, It.IsAny<CancellationToken>()))
            .Returns(Task.CompletedTask);

        var service = new ExternalSearchService(
            metadata.Object,
            new Mock<ILogger<ExternalSearchService>>().Object,
            lastFm);

        var songs = await service.GetAsync("nergal");

        Assert.Equal(50, songs.Count);
        Assert.Equal(1, handler.TrackSearchCalls);
        Assert.Equal(0, handler.TopTracksCalls);
        Assert.True(prefetchOverlappedLastFm);
        metadata.Verify(m => m.PrefetchSearchMetadataAsync(
            "nergal", 25, It.IsAny<CancellationToken>()), Times.Once);
        metadata.Verify(m => m.EnrichExternalSongsAsync(
            It.IsAny<List<Song>>(), It.IsAny<CancellationToken>()), Times.Once);
        metadata.Verify(m => m.ResolveTopDurationsAsync(
            It.IsAny<List<Song>>(), It.IsAny<CancellationToken>()), Times.Never);
        metadata.Verify(m => m.PrewarmYouTubeIdsAsync(
            It.IsAny<IEnumerable<Song>>(), 12, It.IsAny<CancellationToken>()), Times.Once);
    }

    [Fact]
    public async Task FailedMetadataPrefetchFallsBackToExactEnrichment()
    {
        var handler = new LastFmSearchHandler(trackCount: 2);
        var lastFm = new LastFmService(
            new HttpClient(handler),
            Options.Create(new LastFmSettings { ApiKey = "test-key" }),
            Options.Create(new MetadataSettings()),
            new Mock<ILogger<LastFmService>>().Object);

        var metadata = new Mock<IMusicMetadataService>();
        metadata.Setup(m => m.PrefetchSearchMetadataAsync(
                "peja", 25, It.IsAny<CancellationToken>()))
            .ThrowsAsync(new HttpRequestException("Deezer unavailable"));
        metadata.Setup(m => m.SearchSongsByArtistTitleAsync(
                It.IsAny<string>(), It.IsAny<string>(), 1, It.IsAny<int?>()))
            .ReturnsAsync((string artist, string title, int _, int? _) =>
                new List<Song>
                {
                    new()
                    {
                        Id = $"{artist}|{title}",
                        Artist = artist,
                        Title = title,
                        Duration = 180,
                        IsLocal = false,
                    },
                });
        metadata.Setup(m => m.EnrichExternalSongsAsync(
                It.IsAny<List<Song>>(), It.IsAny<CancellationToken>()))
            .Returns(Task.CompletedTask);
        metadata.Setup(m => m.PrewarmYouTubeIdsAsync(
                It.IsAny<IEnumerable<Song>>(), 12, It.IsAny<CancellationToken>()))
            .Returns(Task.CompletedTask);

        var service = new ExternalSearchService(
            metadata.Object,
            new Mock<ILogger<ExternalSearchService>>().Object,
            lastFm);

        var songs = await service.GetAsync("peja");

        Assert.Equal(2, songs.Count);
        metadata.Verify(m => m.EnrichExternalSongsAsync(
            It.IsAny<List<Song>>(), It.IsAny<CancellationToken>()), Times.Once);
    }

    private sealed class LastFmSearchHandler(
        int trackCount,
        Action? trackSearchStarted = null) : HttpMessageHandler
    {
        public int TrackSearchCalls { get; private set; }
        public int TopTracksCalls { get; private set; }

        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken)
        {
            var query = request.RequestUri?.Query ?? "";
            string body;

            if (query.Contains("method=track.search", StringComparison.Ordinal))
            {
                TrackSearchCalls++;
                trackSearchStarted?.Invoke();
                var tracks = Enumerable.Range(1, trackCount)
                    .Select(i => new { name = $"Track {i}", artist = $"Artist {i}" });
                body = JsonSerializer.Serialize(new
                {
                    results = new { trackmatches = new { track = tracks } },
                });
            }
            else
            {
                TopTracksCalls++;
                body = "{\"toptracks\":{\"track\":[]}}";
            }

            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new StringContent(body, Encoding.UTF8, "application/json"),
            });
        }
    }
}
