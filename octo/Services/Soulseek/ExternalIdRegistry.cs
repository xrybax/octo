using System.Collections.Concurrent;
using System.Security.Cryptography;
using System.Text;

namespace Octo.Services.Soulseek;

/// <summary>
/// Server-side registry that maps short opaque IDs (Navidrome-shaped 22-char base62)
/// to Soulseek/YouTube routing info. Subsonic clients are picky about song-id format —
/// some quietly drop entries with long pipe-delimited IDs from their play queues.
/// Translating to a short, alphabetic-looking key avoids that whole class of issue.
///
/// IDs are deterministic (sha256-derived) so the same routing always produces the
/// same id; this keeps caches/de-duplication on the client side stable across calls.
/// The dictionary is LRU-bounded so a single user can't grow it without limit.
/// </summary>
public class ExternalIdRegistry
{
    private const int MaxEntries = 10_000;

    private readonly ConcurrentDictionary<string, SoulseekRouting> _byId = new();
    private readonly LinkedList<string> _lru = new();
    private readonly object _lruLock = new();

    public string Register(SoulseekRouting routing)
    {
        var id = MakeShortId(routing);

        // Legacy/name-only routings still hash together and may be upgraded in place.
        // Precise provider-backed routings have their own ids (see MakeShortId), so two
        // artists named "Feel" can coexist instead of overwriting each other.
        if (_byId.TryGetValue(id, out var existing))
        {
            if (routing.ExternalAlbumId is null && existing.ExternalAlbumId is not null)
                routing.ExternalAlbumId = existing.ExternalAlbumId;
            if (routing.ExternalArtistId is null && existing.ExternalArtistId is not null)
                routing.ExternalArtistId = existing.ExternalArtistId;
            if (routing.ReleaseType is null && existing.ReleaseType is not null)
                routing.ReleaseType = existing.ReleaseType;
            if (routing.CoverArtUrl is null && existing.CoverArtUrl is not null)
                routing.CoverArtUrl = existing.CoverArtUrl;
        }

        _byId[id] = routing;
        Touch(id);
        Trim();
        return id;
    }

    public SoulseekRouting? Lookup(string shortId)
    {
        if (_byId.TryGetValue(shortId, out var r))
        {
            Touch(shortId);
            return r;
        }
        return null;
    }

    private static string MakeShortId(SoulseekRouting r)
    {
        // Derive 22 base62 chars from sha256 of routing fields. Same input -> same id.
        // The Kind prefix is critical: a song "Drake - Hotline Bling" must hash to a
        // different id than the album "Hotline Bling" or the artist "Drake", or
        // getCoverArt would return the wrong scope's artwork.
        var seed = r.Kind switch
        {
            RoutingKind.Album when !string.IsNullOrWhiteSpace(r.ExternalAlbumId)
                => $"k:album|deezer:{r.ExternalAlbumId}",
            RoutingKind.Artist when !string.IsNullOrWhiteSpace(r.ExternalArtistId)
                => $"k:artist|deezer:{r.ExternalArtistId}",
            RoutingKind.Album  => $"k:album|a:{r.Artist}|al:{r.Album}",
            RoutingKind.Artist => $"k:artist|a:{r.Artist}",
            _                  => $"k:song|yt:{r.YouTubeId}|a:{r.Artist}|t:{r.Title}|d:{r.Duration}",
        };
        Span<byte> hash = stackalloc byte[32];
        SHA256.HashData(Encoding.UTF8.GetBytes(seed), hash);
        return ToBase62(hash, 22);
    }

    private const string Alphabet = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

    private static string ToBase62(ReadOnlySpan<byte> bytes, int length)
    {
        // Treat the first 16 bytes as a big integer and base62-encode it. We don't need
        // strict cryptographic uniqueness — just collision resistance within ~10k items.
        var value = new System.Numerics.BigInteger(bytes[..16], isUnsigned: true, isBigEndian: true);
        var sb = new StringBuilder(length);
        var b = new System.Numerics.BigInteger(62);
        while (sb.Length < length)
        {
            value = System.Numerics.BigInteger.DivRem(value, b, out var rem);
            sb.Append(Alphabet[(int)rem]);
            if (value.IsZero) break;
        }
        while (sb.Length < length) sb.Append('0');
        return sb.ToString()[..length];
    }

    private void Touch(string id)
    {
        lock (_lruLock)
        {
            _lru.Remove(id);
            _lru.AddFirst(id);
        }
    }

    private void Trim()
    {
        if (_byId.Count <= MaxEntries) return;
        lock (_lruLock)
        {
            while (_byId.Count > MaxEntries && _lru.Last is not null)
            {
                var oldest = _lru.Last.Value;
                _lru.RemoveLast();
                _byId.TryRemove(oldest, out _);
            }
        }
    }
}
