using System.Globalization;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Microsoft.Extensions.Options;
using Octo.Models.Settings;

namespace Octo.Services.Listening;

public sealed class LastFmScrobblingSink : IListeningSink
{
    public const string SinkName = "lastfm";
    public const string HttpClientName = "lastfm-scrobbling";
    private const string ApiUrl = "https://ws.audioscrobbler.com/2.0/";

    private readonly IHttpClientFactory _httpClientFactory;
    private readonly IOptionsMonitor<LastFmSettings> _settings;

    public LastFmScrobblingSink(
        IHttpClientFactory httpClientFactory,
        IOptionsMonitor<LastFmSettings> settings)
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
                && !string.IsNullOrWhiteSpace(settings.ApiKey)
                && !string.IsNullOrWhiteSpace(settings.ApiSecret)
                && !string.IsNullOrWhiteSpace(settings.SessionKey);
        }
    }

    public Task SendNowPlayingAsync(
        ListeningSubmission submission,
        CancellationToken cancellationToken)
    {
        var parameters = TrackParameters(submission.Track);
        parameters["method"] = "track.updateNowPlaying";
        return PostSignedAsync(parameters, cancellationToken);
    }

    public Task SendScrobbleAsync(
        ListeningSubmission submission,
        CancellationToken cancellationToken)
    {
        var parameters = TrackParameters(submission.Track);
        parameters["method"] = "track.scrobble";
        parameters["timestamp"] = submission.StartedAt.ToUnixTimeSeconds()
            .ToString(CultureInfo.InvariantCulture);
        return PostSignedAsync(parameters, cancellationToken);
    }

    public async Task<string> CreateAuthorizationTokenAsync(CancellationToken cancellationToken)
    {
        var settings = _settings.CurrentValue;
        EnsureApplicationCredentials(settings);
        var parameters = new Dictionary<string, string>(StringComparer.Ordinal)
        {
            ["api_key"] = settings.ApiKey.Trim(),
            ["method"] = "auth.getToken",
        };
        using var document = await PostSignedDocumentAsync(
            parameters, settings.ApiSecret.Trim(), cancellationToken);
        if (!document.RootElement.TryGetProperty("token", out var tokenElement)
            || string.IsNullOrWhiteSpace(tokenElement.GetString()))
        {
            throw new ListeningSinkException("Last.fm did not return an authorization token.", true);
        }
        return tokenElement.GetString()!;
    }

    public string CreateAuthorizationUrl(string token)
    {
        var apiKey = _settings.CurrentValue.ApiKey.Trim();
        return "https://www.last.fm/api/auth/?api_key="
            + Uri.EscapeDataString(apiKey)
            + "&token=" + Uri.EscapeDataString(token);
    }

    public async Task<LastFmSession> ExchangeAuthorizationTokenAsync(
        string token,
        CancellationToken cancellationToken)
    {
        var settings = _settings.CurrentValue;
        EnsureApplicationCredentials(settings);
        if (string.IsNullOrWhiteSpace(token))
            throw new ArgumentException("Authorization token is required.", nameof(token));

        var parameters = new Dictionary<string, string>(StringComparer.Ordinal)
        {
            ["api_key"] = settings.ApiKey.Trim(),
            ["method"] = "auth.getSession",
            ["token"] = token.Trim(),
        };
        using var document = await PostSignedDocumentAsync(
            parameters, settings.ApiSecret.Trim(), cancellationToken);
        if (!document.RootElement.TryGetProperty("session", out var session)
            || !session.TryGetProperty("key", out var keyElement)
            || string.IsNullOrWhiteSpace(keyElement.GetString()))
        {
            throw new ListeningSinkException("Last.fm did not return a session key.", false);
        }

        var username = session.TryGetProperty("name", out var nameElement)
            ? nameElement.GetString() ?? string.Empty
            : string.Empty;
        return new LastFmSession(username, keyElement.GetString()!);
    }

    internal static string CreateApiSignature(
        IEnumerable<KeyValuePair<string, string>> parameters,
        string apiSecret)
    {
        var source = new StringBuilder();
        foreach (var parameter in parameters
                     .Where(parameter => !parameter.Key.Equals("format", StringComparison.Ordinal)
                                      && !parameter.Key.Equals("callback", StringComparison.Ordinal)
                                      && !parameter.Key.Equals("api_sig", StringComparison.Ordinal))
                     .OrderBy(parameter => parameter.Key, StringComparer.Ordinal))
        {
            source.Append(parameter.Key);
            source.Append(parameter.Value);
        }
        source.Append(apiSecret);

        var digest = MD5.HashData(Encoding.UTF8.GetBytes(source.ToString()));
        return Convert.ToHexString(digest).ToLowerInvariant();
    }

    private static Dictionary<string, string> TrackParameters(ListeningTrack track)
    {
        var parameters = new Dictionary<string, string>(StringComparer.Ordinal)
        {
            ["artist"] = track.Artist,
            ["track"] = track.Title,
        };
        if (!string.IsNullOrWhiteSpace(track.Album)) parameters["album"] = track.Album;
        if (!string.IsNullOrWhiteSpace(track.AlbumArtist)) parameters["albumArtist"] = track.AlbumArtist;
        if (track.DurationSeconds is > 0)
            parameters["duration"] = track.DurationSeconds.Value.ToString(CultureInfo.InvariantCulture);
        if (track.TrackNumber is > 0)
            parameters["trackNumber"] = track.TrackNumber.Value.ToString(CultureInfo.InvariantCulture);
        return parameters;
    }

    private async Task PostSignedAsync(
        Dictionary<string, string> parameters,
        CancellationToken cancellationToken)
    {
        var settings = _settings.CurrentValue;
        if (!IsEnabled)
            throw new ListeningSinkException("Last.fm scrobbling is not configured.", true);

        parameters["api_key"] = settings.ApiKey.Trim();
        parameters["sk"] = settings.SessionKey.Trim();
        using var ignored = await PostSignedDocumentAsync(
            parameters, settings.ApiSecret.Trim(), cancellationToken);
    }

    private async Task<JsonDocument> PostSignedDocumentAsync(
        Dictionary<string, string> parameters,
        string apiSecret,
        CancellationToken cancellationToken)
    {
        parameters["api_sig"] = CreateApiSignature(parameters, apiSecret);
        parameters["format"] = "json";

        using var request = new HttpRequestMessage(HttpMethod.Post, ApiUrl)
        {
            Content = new FormUrlEncodedContent(parameters),
        };
        HttpResponseMessage response;
        try
        {
            response = await _httpClientFactory.CreateClient(HttpClientName)
                .SendAsync(request, cancellationToken);
        }
        catch (Exception ex) when (ex is HttpRequestException or TaskCanceledException)
        {
            throw new ListeningSinkException($"Last.fm request failed: {ex.Message}", true, ex);
        }

        using (response)
        {
            var body = await response.Content.ReadAsStringAsync(cancellationToken);
            if (!response.IsSuccessStatusCode)
            {
                var retryable = (int)response.StatusCode >= 500
                    || (int)response.StatusCode == 429;
                throw new ListeningSinkException(
                    $"Last.fm returned HTTP {(int)response.StatusCode}: {body}", retryable);
            }

            JsonDocument document;
            try
            {
                document = JsonDocument.Parse(body);
            }
            catch (JsonException ex)
            {
                throw new ListeningSinkException("Last.fm returned invalid JSON.", true, ex);
            }

            if (document.RootElement.TryGetProperty("error", out var errorElement))
            {
                var code = errorElement.ValueKind == JsonValueKind.Number
                    ? errorElement.GetInt32()
                    : 0;
                var message = document.RootElement.TryGetProperty("message", out var messageElement)
                    ? messageElement.GetString() ?? "unknown error"
                    : "unknown error";
                document.Dispose();
                // 9 can recover after the user reconnects the account. 11 and 16 are
                // Last.fm's documented retry cases; 29 is a temporary rate limit.
                var retryable = code is 9 or 11 or 16 or 29;
                throw new ListeningSinkException(
                    $"Last.fm error {code}: {message}", retryable);
            }
            return document;
        }
    }

    private static void EnsureApplicationCredentials(LastFmSettings settings)
    {
        if (string.IsNullOrWhiteSpace(settings.ApiKey)
            || string.IsNullOrWhiteSpace(settings.ApiSecret))
        {
            throw new InvalidOperationException(
                "Save the Last.fm API key and shared secret before connecting an account.");
        }
    }
}

public sealed record LastFmSession(string Username, string SessionKey);
