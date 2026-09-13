using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Mvc.Testing;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.DependencyInjection.Extensions;
using Microsoft.Extensions.Hosting;
using Moq;
using Octo.Services.Listening;
using Octo.Services.Local;
using Octo.Services.Soulseek;

namespace Octo.Tests;

public sealed class ExternalScrobblingIntegrationTests
{
    private sealed class RecordingQueue : IListeningSubmissionQueue
    {
        public List<ListeningSubmission> NowPlaying { get; } = new();
        public List<ListeningSubmission> Scrobbles { get; } = new();
        public void QueueNowPlaying(ListeningSubmission submission) => NowPlaying.Add(submission);
        public void QueueScrobble(ListeningSubmission submission) => Scrobbles.Add(submission);
    }

    [Fact]
    public async Task TemporaryTrackIsAcknowledgedAndCapturedInsteadOfRelayedToNavidrome()
    {
        var registry = new ExternalIdRegistry();
        var id = registry.Register(new SoulseekRouting
        {
            Artist = "Radiohead",
            Title = "Nude",
            Album = "In Rainbows",
            Duration = 200,
            YouTubeId = "video-id",
        });
        var queue = new RecordingQueue();
        var library = new Mock<ILocalLibraryService>();
        library.Setup(service => service.ParseSongId(It.IsAny<string>()))
            .Returns((string candidate) => candidate == id
                ? (true, "soulseek", id)
                : (false, null, null));

        await using var factory = new WebApplicationFactory<Program>()
            .WithWebHostBuilder(builder =>
            {
                builder.ConfigureAppConfiguration((_, config) =>
                    config.AddInMemoryCollection(new Dictionary<string, string?>
                    {
                        ["Subsonic:Url"] = "http://navidrome.invalid",
                        ["Library:DownloadPath"] = Path.GetTempPath(),
                    }));
                builder.ConfigureServices(services =>
                {
                    services.RemoveAll<IHostedService>();
                    services.RemoveAll<ILocalLibraryService>();
                    services.RemoveAll<ExternalIdRegistry>();
                    services.RemoveAll<IListeningSubmissionQueue>();
                    services.AddSingleton(library.Object);
                    services.AddSingleton(registry);
                    services.AddSingleton<IListeningSubmissionQueue>(queue);
                });
            });

        using var client = factory.CreateClient();
        using var starting = await client.GetAsync(
            $"/rest/reportPlayback?mediaId={id}&mediaType=song&positionMs=0&state=starting&u=mortifer&c=Feishin&f=json");
        using var progress = await client.GetAsync(
            $"/rest/reportPlayback?mediaId={id}&mediaType=song&positionMs=100000&state=playing&u=mortifer&c=Feishin&f=json");
        using var duplicate = await client.GetAsync(
            $"/rest/scrobble?id={id}&submission=true&u=mortifer&c=Feishin&f=json");

        starting.EnsureSuccessStatusCode();
        progress.EnsureSuccessStatusCode();
        duplicate.EnsureSuccessStatusCode();
        Assert.Single(queue.NowPlaying);
        Assert.Single(queue.Scrobbles);
        Assert.Equal("Radiohead", queue.Scrobbles[0].Track.Artist);
        Assert.Equal("Nude", queue.Scrobbles[0].Track.Title);
    }
}
