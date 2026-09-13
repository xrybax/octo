using System.Net;
using System.Net.Http.Json;
using System.Reflection;
using Microsoft.Extensions.Options;
using Octo.Models.Settings;

namespace Octo.Services.Listening;

public sealed class ListenBrainzListeningSink : IListeningSink
{
    public const string SinkName = "listenbrainz";
    public const string HttpClientName = "listenbrainz-scrobbling";

    private readonly IHttpClientFactory _httpClientFactory;
    private readonly IOptionsMonitor<ListenBrainzSettings> _settings;

    public ListenBrainzListeningSink(
        IHttpClientFactory httpClientFactory,
        IOptionsMonitor<ListenBrainzSettings> settings)
    {
        _httpClientFactory = httpClientFactory;
        _settings = settings;
    }

    public string Name => SinkName;

    public bool IsEnabled
    {
        get
        {
            var settings = _settings.CurrentValue;
            return settings.EnableScrobbling
                && !string.IsNullOrWhiteSpace(settings.UserToken)
                && Uri.TryCreate(settings.BaseUrl, UriKind.Absolute, out _);
        }
    }

    public Task SendNowPlayingAsync(
        ListeningSubmission submission,
        CancellationToken cancellationToken)
        => SubmitAsync("playing_now", submission, includeTimestamp: false, cancellationToken);

    public Task SendScrobbleAsync(
        ListeningSubmission submission,
        CancellationToken cancellationToken)
        => SubmitAsync("single", submission, includeTimestamp: true, cancellationToken);

    private async Task SubmitAsync(
        string listenType,
        ListeningSubmission submission,
        bool includeTimestamp,
        CancellationToken cancellationToken)
    {
        var settings = _settings.CurrentValue;
        if (!IsEnabled)
            throw new ListeningSinkException("ListenBrainz scrobbling is not configured.", true);

        var additionalInfo = new Dictionary<string, object>
        {
            ["submission_client"] = "Octo",
            ["submission_client_version"] = Version,
        };
        if (!string.IsNullOrWhiteSpace(submission.MediaPlayer))
            additionalInfo["media_player"] = submission.MediaPlayer;
        if (submission.Track.DurationSeconds is > 0)
            additionalInfo["duration_ms"] = submission.Track.DurationSeconds.Value * 1000L;
        if (submission.Track.TrackNumber is > 0)
            additionalInfo["tracknumber"] = submission.Track.TrackNumber.Value.ToString();
        if (!string.IsNullOrWhiteSpace(submission.Track.Isrc))
            additionalInfo["isrc"] = submission.Track.Isrc;
        if (!string.IsNullOrWhiteSpace(submission.Track.MusicService))
            additionalInfo["music_service"] = submission.Track.MusicService;
        if (!string.IsNullOrWhiteSpace(submission.Track.OriginUrl))
            additionalInfo["origin_url"] = submission.Track.OriginUrl;

        var trackMetadata = new Dictionary<string, object>
        {
            ["artist_name"] = submission.Track.Artist,
            ["track_name"] = submission.Track.Title,
            ["additional_info"] = additionalInfo,
        };
        if (!string.IsNullOrWhiteSpace(submission.Track.Album))
            trackMetadata["release_name"] = submission.Track.Album;

        var listen = new Dictionary<string, object>
        {
            ["track_metadata"] = trackMetadata,
        };
        if (includeTimestamp)
            listen["listened_at"] = submission.StartedAt.ToUnixTimeSeconds();

        var payload = new Dictionary<string, object>
        {
            ["listen_type"] = listenType,
            ["payload"] = new[] { listen },
        };

        var endpoint = settings.BaseUrl.TrimEnd('/') + "/1/submit-listens";
        using var request = new HttpRequestMessage(HttpMethod.Post, endpoint)
        {
            Content = JsonContent.Create(payload),
        };
        request.Headers.TryAddWithoutValidation("Authorization", "Token " + settings.UserToken.Trim());

        HttpResponseMessage response;
        try
        {
            response = await _httpClientFactory.CreateClient(HttpClientName)
                .SendAsync(request, cancellationToken);
        }
        catch (Exception ex) when (ex is HttpRequestException or TaskCanceledException)
        {
            throw new ListeningSinkException($"ListenBrainz request failed: {ex.Message}", true, ex);
        }

        using (response)
        {
            if (response.IsSuccessStatusCode) return;
            var body = await response.Content.ReadAsStringAsync(cancellationToken);
            var retryable = response.StatusCode is HttpStatusCode.Unauthorized
                or HttpStatusCode.RequestTimeout
                or HttpStatusCode.TooManyRequests
                || (int)response.StatusCode >= 500;
            throw new ListeningSinkException(
                $"ListenBrainz returned HTTP {(int)response.StatusCode}: {body}", retryable);
        }
    }

    private static string Version =>
        typeof(ListenBrainzListeningSink).Assembly
            .GetCustomAttribute<AssemblyInformationalVersionAttribute>()?
            .InformationalVersion.Split('+')[0]
        ?? "unknown";
}
