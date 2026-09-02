using Microsoft.Extensions.Logging;
using Moq;
using Moq.Protected;
using Octo.Models.Settings;
using Octo.Services.Metadata;
using System.Net;

namespace Octo.Tests;

public class DeezerMetadataServiceTests
{
    /// <summary>Builds a service whose HTTP layer answers from a url-substring to body map.
    /// Any url with no match returns 404, which exercises the best-effort paths.</summary>
    private static DeezerMetadataService BuildService(Dictionary<string, string> routes,
        string language = "en", List<HttpRequestMessage>? capture = null)
    {
        var handler = new Mock<HttpMessageHandler>();
        handler.Protected()
            .Setup<Task<HttpResponseMessage>>("SendAsync",
                ItExpr.IsAny<HttpRequestMessage>(),
                ItExpr.IsAny<CancellationToken>())
            .ReturnsAsync((HttpRequestMessage req, CancellationToken _) =>
            {
                capture?.Add(req);
                var url = req.RequestUri!.ToString();
                foreach (var (needle, body) in routes)
                {
                    if (url.Contains(needle, StringComparison.OrdinalIgnoreCase))
                        return new HttpResponseMessage(HttpStatusCode.OK)
                        {
                            Content = new StringContent(body),
                        };
                }
                return new HttpResponseMessage(HttpStatusCode.NotFound);
            });

        var factory = new Mock<IHttpClientFactory>();
        factory.Setup(f => f.CreateClient(It.IsAny<string>()))
            .Returns(() => new HttpClient(handler.Object));

        return new DeezerMetadataService(factory.Object,
            TestOptions.Monitor(new MetadataSettings { Language = language }),
            new Mock<ILogger<DeezerMetadataService>>().Object);
    }

    /// <summary>Deezer reports throttling as HTTP 200 with this body, which is the whole
    /// reason a parsed document cannot be treated as a successful call.</summary>
    private const string QuotaEnvelope =
        @"{""error"":{""type"":""Exception"",""message"":""Quota limit exceeded"",""code"":4}}";

    /// <summary>Deezer's only "this really does not exist" answer, also HTTP 200.</summary>
    private const string NoDataEnvelope =
        @"{""error"":{""type"":""DataException"",""message"":""no data"",""code"":800}}";

    /// <summary>Like <see cref="BuildService"/>, but each route serves its bodies in order
    /// and repeats the last one once exhausted, so a test can make a call fail and then
    /// succeed. Routes are matched by url SUBSTRING, so "/album/1/tracks" must be
    /// registered before "/album/1" or the shorter needle swallows both.</summary>
    private static DeezerMetadataService BuildSequencedService(
        List<(string Needle, string[] Bodies)> routes, out Func<string, int> callCount)
    {
        var counts = new Dictionary<string, int>();
        var gate = new object();

        var handler = new Mock<HttpMessageHandler>();
        handler.Protected()
            .Setup<Task<HttpResponseMessage>>("SendAsync",
                ItExpr.IsAny<HttpRequestMessage>(),
                ItExpr.IsAny<CancellationToken>())
            .ReturnsAsync((HttpRequestMessage req, CancellationToken _) =>
            {
                var url = req.RequestUri!.ToString();
                lock (gate)
                {
                    foreach (var (needle, bodies) in routes)
                    {
                        if (!url.Contains(needle, StringComparison.OrdinalIgnoreCase)) continue;
                        counts.TryGetValue(needle, out var n);
                        counts[needle] = n + 1;
                        return new HttpResponseMessage(HttpStatusCode.OK)
                        {
                            Content = new StringContent(bodies[Math.Min(n, bodies.Length - 1)]),
                        };
                    }
                }
                return new HttpResponseMessage(HttpStatusCode.NotFound);
            });

        var factory = new Mock<IHttpClientFactory>();
        factory.Setup(f => f.CreateClient(It.IsAny<string>()))
            .Returns(() => new HttpClient(handler.Object));

        callCount = needle => { lock (gate) { return counts.TryGetValue(needle, out var n) ? n : 0; } };
        return new DeezerMetadataService(factory.Object,
            TestOptions.Monitor(new MetadataSettings()),
            new Mock<ILogger<DeezerMetadataService>>().Object);
    }

    private const string AlbumDetailJson =
        @"{""id"":711108,""title"":""Canciones Prohibidas"",""nb_tracks"":10,""label"":""WM Spain"",
           ""release_date"":""1998-04-30"",""cover_xl"":""https://cdn/xl.jpg"",
           ""artist"":{""name"":""Extremoduro""},""genres"":{""data"":[{""name"":""Pop""}]}}";

    private static string TracksJson(int count) =>
        @"{""total"":" + count + @",""data"":[" + string.Join(",",
            Enumerable.Range(1, count).Select(i =>
                $@"{{""title"":""Track {i}"",""duration"":200,""track_position"":{i},
                     ""disk_number"":1,""artist"":{{""name"":""Extremoduro""}}}}")) + "]}";

    [Fact]
    public async Task SearchAlbumsAsync_MapsFieldsAndDropsSingles()
    {
        // Arrange: one real album, one EP, and a one-track "single" that must be dropped.
        var json = @"{""data"":[
            {""id"":14880659,""title"":""In Rainbows"",""record_type"":""album"",""nb_tracks"":10,
             ""cover_xl"":""https://cdn/xl.jpg"",""artist"":{""name"":""Radiohead""}},
            {""id"":14880561,""title"":""In Rainbows (Disk 2)"",""record_type"":""ep"",""nb_tracks"":8,
             ""cover_xl"":""https://cdn/ep.jpg"",""artist"":{""name"":""Radiohead""}},
            {""id"":999,""title"":""Nude"",""record_type"":""single"",""nb_tracks"":1,
             ""cover_xl"":""https://cdn/s.jpg"",""artist"":{""name"":""Radiohead""}}
        ]}";
        var svc = BuildService(new() { ["/search/album"] = json });

        // Act
        var hits = await svc.SearchAlbumsAsync("In Rainbows", 10);

        // Assert
        Assert.Equal(2, hits.Count);
        Assert.Equal("14880659", hits[0].DeezerId);
        Assert.Equal("In Rainbows", hits[0].Title);
        Assert.Equal("Radiohead", hits[0].Artist);
        Assert.Equal("https://cdn/xl.jpg", hits[0].CoverUrl);
        Assert.Equal(10, hits[0].TrackCount);
        Assert.DoesNotContain(hits, h => h.Title == "Nude");
    }

    [Fact]
    public async Task SearchAlbumsAsync_KeepsMultiTrackSingle()
    {
        // Only a 1-2 track "single" is noise; a longer one is a real release.
        var json = @"{""data"":[{""id"":5,""title"":""Long Single"",""record_type"":""single"",
            ""nb_tracks"":6,""artist"":{""name"":""X""}}]}";
        var svc = BuildService(new() { ["/search/album"] = json });

        var hits = await svc.SearchAlbumsAsync("q", 10);

        Assert.Single(hits);
    }

    [Fact]
    public async Task SearchArtistsAsync_MapsStableIdImageAndAlbumCount()
    {
        var json = @"{""data"":[
            {""id"":399,""name"":""Radiohead"",""nb_album"":42,
             ""picture_xl"":""https://cdn/radiohead.jpg""},
            {""id"":400,""name"":""Radio Head"",""nb_album"":3}
        ]}";
        var svc = BuildService(new() { ["/search/artist"] = json });

        var artists = await svc.SearchArtistsAsync("Radiohead", 5);

        Assert.Equal(2, artists.Count);
        Assert.Equal("399", artists[0].DeezerId);
        Assert.Equal("Radiohead", artists[0].Name);
        Assert.Equal("https://cdn/radiohead.jpg", artists[0].ImageUrl);
        Assert.Equal(42, artists[0].AlbumCount);
    }

    [Fact]
    public async Task GetArtistAlbumsAsync_KeepsAlbumsEpsAndSingles()
    {
        // Artist browsing is deliberately different from global album search: singles
        // are part of a discography and must not be filtered out.
        var json = @"{""total"":3,""data"":[
            {""id"":1,""title"":""LP"",""record_type"":""album"",""nb_tracks"":10,
             ""release_date"":""2020-01-02"",""cover_xl"":""https://cdn/lp.jpg""},
            {""id"":2,""title"":""Small EP"",""record_type"":""ep"",""nb_tracks"":4,
             ""release_date"":""2021-03-04""},
            {""id"":3,""title"":""One Song"",""record_type"":""single"",""nb_tracks"":1,
             ""release_date"":""2022-05-06""}
        ]}";
        var svc = BuildService(new() { ["/artist/399/albums"] = json });

        var albums = await svc.GetArtistAlbumsAsync("399", "Radiohead");

        Assert.Equal(3, albums.Count);
        Assert.Equal(new[] { "album", "ep", "single" }, albums.Select(a => a.RecordType!));
        Assert.Equal(new int?[] { 2020, 2021, 2022 }, albums.Select(a => a.Year));
        Assert.All(albums, album =>
        {
            Assert.Equal("Radiohead", album.Artist);
            Assert.Equal("399", album.ArtistDeezerId);
        });
    }

    [Fact]
    public async Task GetArtistAlbumsAsync_FollowsPagination()
    {
        var pageOne = @"{""total"":101,""data"":[" + string.Join(",",
            Enumerable.Range(1, 100).Select(i =>
                $@"{{""id"":{i},""title"":""Release {i}"",""record_type"":""album""}}")) + "]}";
        var pageTwo = @"{""total"":101,""data"":[
            {""id"":101,""title"":""Release 101"",""record_type"":""single""}
        ]}";
        var svc = BuildService(new()
        {
            ["index=0"] = pageOne,
            ["index=100"] = pageTwo,
        });

        var albums = await svc.GetArtistAlbumsAsync("399", "Radiohead", 101);

        Assert.Equal(101, albums.Count);
        Assert.Equal("101", albums[^1].DeezerId);
    }

    [Fact]
    public async Task GetArtistAlbumsAsync_ThrottledLaterPage_DoesNotCachePartialList()
    {
        var pageOne = @"{""total"":101,""data"":[" + string.Join(",",
            Enumerable.Range(1, 100).Select(i =>
                $@"{{""id"":{i},""title"":""Release {i}"",""record_type"":""album""}}")) + "]}";
        const string pageTwo = @"{""total"":101,""data"":[
            {""id"":101,""title"":""Release 101"",""record_type"":""single""}
        ]}";
        var svc = BuildSequencedService(new()
        {
            ("index=0", new[] { pageOne }),
            ("index=100", new[] { QuotaEnvelope, pageTwo }),
        }, out _);

        Assert.Empty(await svc.GetArtistAlbumsAsync("399", "Radiohead", 101));

        var recovered = await svc.GetArtistAlbumsAsync("399", "Radiohead", 101);
        Assert.Equal(101, recovered.Count);
    }

    [Fact]
    public async Task GetAlbumDetailAsync_OrdersByDiscThenTrackPosition()
    {
        // Arrange: deliberately out of order, spanning two discs.
        var album = @"{""id"":1,""title"":""Test Album"",""cover_xl"":""https://cdn/a.jpg"",
            ""release_date"":""1997-05-21"",""label"":""Label X"",
            ""artist"":{""name"":""Test Artist""},""genres"":{""data"":[{""name"":""Rock""}]}}";
        var tracks = @"{""total"":4,""data"":[
            {""title"":""D2T1"",""duration"":100,""track_position"":1,""disk_number"":2,""artist"":{""name"":""Test Artist""}},
            {""title"":""D1T2"",""duration"":200,""track_position"":2,""disk_number"":1,""isrc"":""ABC"",""artist"":{""name"":""Test Artist""}},
            {""title"":""D1T1"",""duration"":300,""track_position"":1,""disk_number"":1,""artist"":{""name"":""Test Artist""}},
            {""title"":""D2T2"",""duration"":150,""track_position"":2,""disk_number"":2,""artist"":{""name"":""Test Artist""}}
        ]}";
        var svc = BuildService(new()
        {
            ["/album/1/tracks"] = tracks,
            ["/album/1"] = album,
        });

        // Act
        var detail = await svc.GetAlbumDetailAsync("1");

        // Assert
        Assert.NotNull(detail);
        Assert.Equal("Test Album", detail!.Title);
        Assert.Equal("Test Artist", detail.Artist);
        Assert.Equal(1997, detail.Year);
        Assert.Equal("Rock", detail.Genre);
        Assert.Equal("Label X", detail.Label);
        Assert.Equal(new[] { "D1T1", "D1T2", "D2T1", "D2T2" }, detail.Tracks.Select(t => t.Title));
        Assert.Equal("ABC", detail.Tracks[1].Isrc);
        Assert.Equal(300, detail.Tracks[0].Duration);
    }

    [Fact]
    public async Task GetAlbumDetailAsync_MalformedPayload_ReturnsNullWithoutThrowing()
    {
        var svc = BuildService(new() { ["/album/"] = "{ this is not json" });

        var detail = await svc.GetAlbumDetailAsync("1");

        Assert.Null(detail);
    }

    [Fact]
    public async Task GetAlbumDetailAsync_UnreachableApi_ReturnsNull()
    {
        // No routes registered, so every request 404s.
        var svc = BuildService(new());

        Assert.Null(await svc.GetAlbumDetailAsync("1"));
    }

    [Fact]
    public async Task FindAlbumIdAsync_ReturnsFirstMatch()
    {
        var json = @"{""data"":[{""id"":14880659,""title"":""In Rainbows""}]}";
        var svc = BuildService(new() { ["/search/album"] = json });

        var id = await svc.FindAlbumIdAsync("Radiohead", "In Rainbows");

        Assert.Equal("14880659", id);
    }

    [Fact]
    public async Task FindAlbumIdAsync_NoAlbumName_ReturnsNullWithoutCallingApi()
    {
        var svc = BuildService(new());

        Assert.Null(await svc.FindAlbumIdAsync("Radiohead", ""));
    }

    [Fact]
    public async Task SearchAlbumsAsync_EmptyQueryOrZeroLimit_ReturnsEmpty()
    {
        var svc = BuildService(new());

        Assert.Empty(await svc.SearchAlbumsAsync("", 10));
        Assert.Empty(await svc.SearchAlbumsAsync("q", 0));
    }

    // ---- Quota-poisoning regression tests (issue #8) ------------------------
    // Deezer answers HTTP 200 even when refusing a call, so a parsed document is not
    // a successful call. Caching one of those refusals is what made an album report
    // songCount 0 permanently.

    /// <summary>
    /// The exact reported failure: the album call succeeds and the TRACKLIST call is
    /// throttled, which used to build a valid AlbumDetail carrying real title/year/genre
    /// with an empty tracklist and cache it forever.
    /// </summary>
    [Fact]
    public async Task GetAlbumDetailAsync_TracklistThrottled_NotCachedAndRecoversOnRetry()
    {
        var svc = BuildSequencedService(new()
        {
            // Longest needle first: "/album/711108" is a prefix of the tracks url.
            ("/album/711108/tracks", new[] { QuotaEnvelope, TracksJson(10) }),
            ("/album/711108", new[] { AlbumDetailJson }),
        }, out var calls);

        var first = await svc.GetAlbumDetailAsync("711108");
        Assert.Null(first);

        var second = await svc.GetAlbumDetailAsync("711108");
        Assert.NotNull(second);
        Assert.Equal(10, second!.Tracks.Count);
        Assert.Equal("Canciones Prohibidas", second.Title);
        Assert.Equal(1998, second.Year);

        // Proves it retried rather than serving a cached failure.
        Assert.Equal(2, calls("/album/711108/tracks"));
    }

    /// <summary>The other poisoning branch: the album call itself is throttled.</summary>
    [Fact]
    public async Task GetAlbumDetailAsync_AlbumCallThrottled_NotCachedAndRecoversOnRetry()
    {
        var svc = BuildSequencedService(new()
        {
            ("/album/711108/tracks", new[] { TracksJson(10) }),
            ("/album/711108", new[] { QuotaEnvelope, AlbumDetailJson }),
        }, out _);

        Assert.Null(await svc.GetAlbumDetailAsync("711108"));

        var second = await svc.GetAlbumDetailAsync("711108");
        Assert.NotNull(second);
        Assert.Equal(10, second!.Tracks.Count);
    }

    /// <summary>
    /// Int() returns int?, and a lifted "nbTracks > 0" is FALSE when nb_tracks is absent.
    /// Testing that alone would let an empty tracklist through and cache it, leaving the
    /// reported bug fixed only for albums that happen to report a track count.
    /// </summary>
    [Fact]
    public async Task GetAlbumDetailAsync_EmptyTracklistAndNoTrackCount_NotCached()
    {
        const string noNbTracks =
            @"{""id"":711108,""title"":""Canciones Prohibidas"",""artist"":{""name"":""Extremoduro""}}";

        var svc = BuildSequencedService(new()
        {
            ("/album/711108/tracks", new[] { @"{""data"":[]}", TracksJson(10) }),
            ("/album/711108", new[] { noNbTracks, AlbumDetailJson }),
        }, out _);

        Assert.Null(await svc.GetAlbumDetailAsync("711108"));

        var second = await svc.GetAlbumDetailAsync("711108");
        Assert.NotNull(second);
        Assert.Equal(10, second!.Tracks.Count);
    }

    /// <summary>
    /// The case the guard must NOT break: Deezer says the album has zero tracks and
    /// returns zero tracks. That is an answer, so it is cached rather than refetched
    /// on every getAlbum forever.
    /// </summary>
    [Fact]
    public async Task GetAlbumDetailAsync_GenuinelyEmptyAlbum_IsAnsweredAndCached()
    {
        const string zeroTracks =
            @"{""id"":42,""title"":""Empty"",""nb_tracks"":0,""artist"":{""name"":""Nobody""}}";

        var svc = BuildSequencedService(new()
        {
            ("/album/42/tracks", new[] { @"{""total"":0,""data"":[]}" }),
            ("/album/42", new[] { zeroTracks }),
        }, out var calls);

        var first = await svc.GetAlbumDetailAsync("42");
        Assert.NotNull(first);
        Assert.Empty(first!.Tracks);

        await svc.GetAlbumDetailAsync("42");
        Assert.Equal(1, calls("/album/42/tracks"));
    }

    /// <summary>A definitive "no data" is cacheable, unlike a throttle.</summary>
    [Fact]
    public async Task GetAlbumDetailAsync_DefinitiveNoData_IsCached()
    {
        var svc = BuildSequencedService(new()
        {
            ("/album/999/tracks", new[] { TracksJson(3) }),
            ("/album/999", new[] { NoDataEnvelope, AlbumDetailJson }),
        }, out var calls);

        Assert.Null(await svc.GetAlbumDetailAsync("999"));
        Assert.Null(await svc.GetAlbumDetailAsync("999"));

        // One call only: a definitive answer is allowed to stick.
        Assert.Equal(1, calls("/album/999"));
    }

    /// <summary>
    /// A throttled album search used to cache an empty list, so external albums silently
    /// stopped appearing in search3 for the life of the process.
    /// </summary>
    [Fact]
    public async Task SearchAlbumsAsync_Throttled_NotCachedAndRecoversOnRetry()
    {
        const string results =
            @"{""data"":[{""id"":711108,""title"":""Canciones Prohibidas"",""record_type"":""album"",
               ""nb_tracks"":10,""cover_xl"":""https://cdn/xl.jpg"",""artist"":{""name"":""Extremoduro""}}]}";

        var svc = BuildSequencedService(new()
        {
            ("/search/album", new[] { QuotaEnvelope, results }),
        }, out _);

        Assert.Empty(await svc.SearchAlbumsAsync("extremoduro golfa", 10));

        var second = await svc.SearchAlbumsAsync("extremoduro golfa", 10);
        Assert.Single(second);
        Assert.Equal("711108", second[0].DeezerId);
    }

    /// <summary>
    /// Entries now expire on their own, but a definitive negative still sticks for its TTL.
    /// ClearCaches is the lever that turns "wait for the TTL" into "fixed now", so it has
    /// to actually drop cached answers.
    /// </summary>
    [Fact]
    public async Task ClearCaches_ForcesARefetch()
    {
        var svc = BuildSequencedService(new()
        {
            ("/album/999/tracks", new[] { TracksJson(3) }),
            ("/album/999", new[] { NoDataEnvelope, AlbumDetailJson }),
        }, out var calls);

        Assert.Null(await svc.GetAlbumDetailAsync("999"));
        Assert.Null(await svc.GetAlbumDetailAsync("999"));
        Assert.Equal(1, calls("/album/999"));

        svc.ClearCaches();

        Assert.NotNull(await svc.GetAlbumDetailAsync("999"));
        Assert.Equal(2, calls("/album/999"));
    }

    // ---- Metadata language (issue #24) ---------------------------------------
    // Deezer localizes genre names to the caller's IP country, so the request
    // must pin the language or a non-English host writes localized genre tags.

    [Fact]
    public async Task Requests_CarryConfiguredAcceptLanguage()
    {
        var seen = new List<HttpRequestMessage>();
        var svc = BuildService(new(), language: "en", capture: seen);

        await svc.GetAlbumDetailAsync("1");

        Assert.NotEmpty(seen);
        Assert.All(seen, r => Assert.Contains(r.Headers.AcceptLanguage, v => v.Value == "en"));
    }

    [Fact]
    public async Task Requests_OmitAcceptLanguage_WhenLanguageEmpty()
    {
        var seen = new List<HttpRequestMessage>();
        var svc = BuildService(new(), language: "", capture: seen);

        await svc.GetAlbumDetailAsync("1");

        Assert.NotEmpty(seen);
        Assert.All(seen, r => Assert.Empty(r.Headers.AcceptLanguage));
    }

    /// <summary>A throttled track search must not be cached as "this track has no metadata".</summary>
    [Fact]
    public async Task EnrichTrackAsync_Throttled_NotCachedAndRecoversOnRetry()
    {
        const string track =
            @"{""data"":[{""title"":""Golfa"",""duration"":359,
               ""album"":{""id"":711108,""title"":""Canciones Prohibidas"",""cover_xl"":""https://cdn/xl.jpg""},
               ""artist"":{""name"":""Extremoduro""}}]}";

        var svc = BuildSequencedService(new()
        {
            ("/search?q=", new[] { QuotaEnvelope, track }),
            ("/album/711108", new[] { AlbumDetailJson }),
        }, out _);

        Assert.Null(await svc.EnrichTrackAsync("Extremoduro", "Golfa"));

        var second = await svc.EnrichTrackAsync("Extremoduro", "Golfa");
        Assert.NotNull(second);
        Assert.Equal("Canciones Prohibidas", second!.AlbumTitle);
    }
}
