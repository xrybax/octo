using System.Text;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Logging.Abstractions;
using Moq;
using Octo.Models.Domain;
using Octo.Models.Settings;
using Octo.Services;
using Octo.Services.Common;
using Octo.Services.Local;
using Octo.Services.Metadata;
using Octo.Services.Soulseek;
using Octo.Services.Subsonic;

namespace Octo.Tests;

public sealed class AlbumDownloadMetadataTests
{
    [Theory]
    [InlineData(2024)]
    [InlineData(null)]
    public async Task TaggingAlbumTracks_KeepsOneReleaseAndDoesNotSearchOtherEditions(int? year)
    {
        var directory = Path.Combine(Path.GetTempPath(), "octo-album-tags-" + Guid.NewGuid());
        var services = new Mock<IServiceProvider>();
        var download = new TaggingDownload(directory, services.Object);
        var release = new AlbumReleaseMetadata("123", "42", "Best Of", "Feel",
            year, "Pop", null, "Label", 2);
        try
        {
            for (var i = 1; i <= 2; i++)
            {
                var path = Path.Combine(directory, $"{i}.wav");
                WriteWave(path);
                using (var original = TagLib.File.Create(path))
                {
                    original.Tag.Album = $"Original album {i}";
                    original.Tag.AlbumArtists = new[] { $"Other album artist {i}" };
                    original.Tag.Year = (uint)(1990 + i);
                    original.Save();
                }
                var song = new Song
                {
                    Title = $"Song {i}", Artist = $"Feel feat. Guest {i}",
                    Album = $"Wrong edition {i}", AlbumArtist = "Wrong", Year = 1999,
                    Track = i, DiscNumber = 1, Release = release,
                };

                await download.Tag(song, path);

                using var tagged = TagLib.File.Create(path);
                Assert.Equal("Best Of", tagged.Tag.Album);
                Assert.Equal(new[] { "Feel" }, tagged.Tag.AlbumArtists);
                Assert.Equal(new[] { song.Artist }, tagged.Tag.Performers);
                Assert.Equal((uint)(year ?? 0), tagged.Tag.Year);
                Assert.Equal((uint)i, tagged.Tag.Track);
                Assert.Equal(2u, tagged.Tag.TrackCount);
                Assert.Equal(1u, tagged.Tag.Disc);
            }
            services.Verify(s => s.GetService(typeof(DeezerMetadataService)), Times.Never);
        }
        finally { Directory.Delete(directory, recursive: true); }
    }

    [Fact]
    public void OrganizedAlbumPaths_ShareAlbumArtistAndDoNotOverwriteAnotherDisc()
    {
        var release = new AlbumReleaseMetadata("123", "42", "Best Of", "Feel",
            2024, null, null, null, 2);
        var first = new SoulseekRouting
        {
            Artist = "Feel", Album = "Wrong original album", Title = "Intro",
            Track = 1, DiscNumber = 1, Release = release,
        };
        var second = new SoulseekRouting
        {
            Artist = "Feel feat. Guest", Album = "Another original album", Title = "Intro",
            Track = 1, DiscNumber = 2, Release = release,
        };
        var root = Path.Combine(Path.GetTempPath(), "octo-paths");
        var pathOne = SoulseekDownloadService.BuildOrganizedPath(root, first, first.Artist!, "Intro", ".m4a");
        var pathTwo = SoulseekDownloadService.BuildOrganizedPath(root, second, second.Artist!, "Intro", ".m4a");

        Assert.Equal(Path.Combine(root, "Feel", "Best Of"), Path.GetDirectoryName(pathOne));
        Assert.Equal(Path.GetDirectoryName(pathOne), Path.GetDirectoryName(pathTwo));
        Assert.NotEqual(pathOne, pathTwo);
        Assert.EndsWith(".m4a", pathTwo);
    }

    private static void WriteWave(string path)
    {
        using var writer = new BinaryWriter(File.Create(path));
        writer.Write(Encoding.ASCII.GetBytes("RIFF")); writer.Write(36 + 1600);
        writer.Write(Encoding.ASCII.GetBytes("WAVEfmt ")); writer.Write(16);
        writer.Write((short)1); writer.Write((short)1); writer.Write(8000);
        writer.Write(16000); writer.Write((short)2); writer.Write((short)16);
        writer.Write(Encoding.ASCII.GetBytes("data")); writer.Write(1600);
        writer.Write(new byte[1600]);
    }

    // Exercise the production tagger without transfers, notifications, or a server.
    private sealed class TaggingDownload : BaseDownloadService
    {
        public TaggingDownload(string directory, IServiceProvider services)
            : base(new ConfigurationBuilder().AddInMemoryCollection(new Dictionary<string, string?>
                { ["Library:DownloadPath"] = directory }).Build(),
                Mock.Of<ILocalLibraryService>(), Mock.Of<IMusicMetadataService>(),
                TestOptions.Monitor(new SubsonicSettings()),
                new NavidromeIdentityService(TestOptions.Monitor(new SubsonicSettings()),
                    Mock.Of<IHttpClientFactory>(), NullLogger<NavidromeIdentityService>.Instance),
                null!, null!, services, NullLogger.Instance) { }

        public Task Tag(Song song, string path) => EnrichAndTagAsync(song, path, CancellationToken.None);
        protected override string ProviderName => "test";
        public override Task<bool> IsAvailableAsync() => Task.FromResult(true);
        protected override string? ExtractExternalIdFromAlbumId(string id) => id;
        protected override Task<string> DownloadTrackAsync(string id, Song song, bool suppressNotify,
            DownloadSource? sourceOverride, CancellationToken ct) => throw new NotSupportedException();
    }
}
