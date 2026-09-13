using System.Net;
using System.Text.Json.Nodes;
using Microsoft.AspNetCore.WebUtilities;
using Moq;
using Octo.Models.Settings;
using Octo.Services.Listening;

namespace Octo.Tests;

public sealed class ListeningSinkTests
{
    private sealed class CapturingHandler : HttpMessageHandler
    {
        public List<CapturedRequest> Requests { get; } = new();
        public Func<CapturedRequest, HttpResponseMessage> Respond { get; set; } = _ =>
            new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent("{}") };

        protected override async Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken)
        {
            var captured = new CapturedRequest(
                request.Method,
                request.RequestUri!,
                request.Headers.TryGetValues("Authorization", out var values)
                    ? values.Single()
                    : null,
                request.Content is null
                    ? null
                    : await request.Content.ReadAsStringAsync(cancellationToken));
            Requests.Add(captured);
            return Respond(captured);
        }
    }

    private sealed record CapturedRequest(
        HttpMethod Method,
        Uri Uri,
        string? Authorization,
        string? Body);

    [Fact]
    public void LastFmSignatureMatchesTheOfficialAuthenticationExample()
    {
        var parameters = new Dictionary<string, string>
        {
            ["token"] = "yyyyyy",
            ["method"] = "auth.getSession",
            ["api_key"] = "xxxxxxxxxx",
            ["format"] = "json",
        };

        var signature = LastFmScrobblingSink.CreateApiSignature(parameters, "ilovecher");

        Assert.Equal("b87d61da3cda91a8b6746c4aef55d6f8", signature);
    }

    [Fact]
    public async Task LastFmScrobblePostsSignedMetadataAndStartTimestamp()
    {
        var handler = new CapturingHandler();
        var settings = TestOptions.Monitor(new LastFmSettings
        {
            ApiKey = "api-key",
            ApiSecret = "api-secret",
            SessionKey = "session-key",
            EnableScrobbling = true,
        });
        var sink = new LastFmScrobblingSink(Factory(handler), settings);

        await sink.SendScrobbleAsync(Submission(), CancellationToken.None);

        var request = Assert.Single(handler.Requests);
        Assert.Equal(HttpMethod.Post, request.Method);
        Assert.Equal("https://ws.audioscrobbler.com/2.0/", request.Uri.ToString());
        var form = QueryHelpers.ParseQuery("?" + request.Body);
        Assert.Equal("track.scrobble", form["method"].ToString());
        Assert.Equal("Radiohead", form["artist"].ToString());
        Assert.Equal("Nude", form["track"].ToString());
        Assert.Equal("In Rainbows", form["album"].ToString());
        Assert.Equal("200", form["duration"].ToString());
        Assert.Equal("3", form["trackNumber"].ToString());
        Assert.Equal(Submission().StartedAt.ToUnixTimeSeconds().ToString(), form["timestamp"].ToString());
        Assert.Equal("api-key", form["api_key"].ToString());
        Assert.Equal("session-key", form["sk"].ToString());
        Assert.Equal("json", form["format"].ToString());
        Assert.Equal(32, form["api_sig"].ToString().Length);
    }

    [Fact]
    public async Task LastFmAuthorizationCreatesAndExchangesDesktopToken()
    {
        var handler = new CapturingHandler
        {
            Respond = request =>
            {
                var form = QueryHelpers.ParseQuery("?" + request.Body);
                var body = form["method"].ToString() == "auth.getToken"
                    ? "{\"token\":\"short-token\"}"
                    : "{\"session\":{\"name\":\"mortifer\",\"key\":\"long-session\"}}";
                return new HttpResponseMessage(HttpStatusCode.OK)
                {
                    Content = new StringContent(body),
                };
            },
        };
        var settings = TestOptions.Monitor(new LastFmSettings
        {
            ApiKey = "api-key",
            ApiSecret = "api-secret",
        });
        var sink = new LastFmScrobblingSink(Factory(handler), settings);

        var token = await sink.CreateAuthorizationTokenAsync(CancellationToken.None);
        var session = await sink.ExchangeAuthorizationTokenAsync(token, CancellationToken.None);

        Assert.Equal("short-token", token);
        Assert.Equal("mortifer", session.Username);
        Assert.Equal("long-session", session.SessionKey);
        Assert.Contains("api_key=api-key", sink.CreateAuthorizationUrl(token));
        Assert.Contains("token=short-token", sink.CreateAuthorizationUrl(token));
    }

    [Fact]
    public async Task ListenBrainzUsesTokenAndRequiredSubmissionShape()
    {
        var handler = new CapturingHandler();
        var settings = TestOptions.Monitor(new ListenBrainzSettings
        {
            BaseUrl = "https://api.listenbrainz.org",
            UserToken = "user-token",
            EnableScrobbling = true,
        });
        var sink = new ListenBrainzListeningSink(Factory(handler), settings);

        await sink.SendNowPlayingAsync(Submission(), CancellationToken.None);
        await sink.SendScrobbleAsync(Submission(), CancellationToken.None);

        Assert.Equal(2, handler.Requests.Count);
        Assert.All(handler.Requests, request =>
        {
            Assert.Equal("Token user-token", request.Authorization);
            Assert.Equal("/1/submit-listens", request.Uri.AbsolutePath);
        });

        var playingNow = JsonNode.Parse(handler.Requests[0].Body!)!;
        Assert.Equal("playing_now", playingNow["listen_type"]!.GetValue<string>());
        Assert.Null(playingNow["payload"]![0]!["listened_at"]);

        var single = JsonNode.Parse(handler.Requests[1].Body!)!;
        Assert.Equal("single", single["listen_type"]!.GetValue<string>());
        var listen = single["payload"]![0]!;
        Assert.Equal(Submission().StartedAt.ToUnixTimeSeconds(), listen["listened_at"]!.GetValue<long>());
        var metadata = listen["track_metadata"]!;
        Assert.Equal("Radiohead", metadata["artist_name"]!.GetValue<string>());
        Assert.Equal("Nude", metadata["track_name"]!.GetValue<string>());
        Assert.Equal("In Rainbows", metadata["release_name"]!.GetValue<string>());
        var additional = metadata["additional_info"]!;
        Assert.Equal(200_000, additional["duration_ms"]!.GetValue<long>());
        Assert.Equal("GBSTK0700001", additional["isrc"]!.GetValue<string>());
        Assert.Equal("youtube.com", additional["music_service"]!.GetValue<string>());
        Assert.Equal("https://www.youtube.com/watch?v=video-id", additional["origin_url"]!.GetValue<string>());
        Assert.Equal("Octo", additional["submission_client"]!.GetValue<string>());
    }

    [Fact]
    public void SinksReactToHotReloadedCredentials()
    {
        var lastFm = TestOptions.Monitor(new LastFmSettings());
        var listenBrainz = TestOptions.Monitor(new ListenBrainzSettings());
        var handler = new CapturingHandler();
        var lastFmSink = new LastFmScrobblingSink(Factory(handler), lastFm);
        var listenBrainzSink = new ListenBrainzListeningSink(Factory(handler), listenBrainz);
        Assert.False(lastFmSink.IsEnabled);
        Assert.False(listenBrainzSink.IsEnabled);

        lastFm.Set(new LastFmSettings
        {
            EnableScrobbling = true,
            ApiKey = "key",
            ApiSecret = "secret",
            SessionKey = "session",
        });
        listenBrainz.Set(new ListenBrainzSettings
        {
            EnableScrobbling = true,
            UserToken = "token",
        });

        Assert.True(lastFmSink.IsEnabled);
        Assert.True(listenBrainzSink.IsEnabled);
    }

    private static IHttpClientFactory Factory(HttpMessageHandler handler)
    {
        var factory = new Mock<IHttpClientFactory>();
        factory.Setup(value => value.CreateClient(It.IsAny<string>()))
            .Returns(() => new HttpClient(handler, disposeHandler: false));
        return factory.Object;
    }

    private static ListeningSubmission Submission() => new()
    {
        StartedAt = new DateTimeOffset(2026, 9, 13, 12, 0, 0, TimeSpan.Zero),
        MediaPlayer = "Feishin",
        Track = new ListeningTrack
        {
            MediaId = "temporary-id",
            Artist = "Radiohead",
            Title = "Nude",
            Album = "In Rainbows",
            AlbumArtist = "Radiohead",
            DurationSeconds = 200,
            TrackNumber = 3,
            Isrc = "GBSTK0700001",
            MusicService = "youtube.com",
            OriginUrl = "https://www.youtube.com/watch?v=video-id",
        },
    };
}
