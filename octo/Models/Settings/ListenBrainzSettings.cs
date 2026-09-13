namespace Octo.Models.Settings;

public sealed class ListenBrainzSettings
{
    public bool EnableScrobbling { get; set; }
    public string UserToken { get; set; } = string.Empty;
    public string BaseUrl { get; set; } = "https://api.listenbrainz.org";
}
