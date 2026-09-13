namespace Octo.Models.Settings;

public class LastFmSettings
{
    /// <summary>
    /// Last.fm API key for fetching similar tracks
    /// </summary>
    public string ApiKey { get; set; } = string.Empty;

    /// <summary>Shared secret used to sign authenticated scrobbling calls.</summary>
    public string ApiSecret { get; set; } = string.Empty;

    /// <summary>Long-lived key granted after the user authorizes Octo.</summary>
    public string SessionKey { get; set; } = string.Empty;

    /// <summary>Informational account name returned with the session key.</summary>
    public string Username { get; set; } = string.Empty;

    /// <summary>Send Octo-only temporary-track listens to Last.fm.</summary>
    public bool EnableScrobbling { get; set; }
    
    /// <summary>
    /// Enable/disable the radio feature
    /// </summary>
    public bool EnableRadio { get; set; } = true;
    
    /// <summary>
    /// Number of similar tracks to return
    /// </summary>
    public int RadioTrackCount { get; set; } = 50;
    
    /// <summary>
    /// Cache duration for Last.fm lookups in hours
    /// </summary>
    public int RadioCacheDurationHours { get; set; } = 24;
}
