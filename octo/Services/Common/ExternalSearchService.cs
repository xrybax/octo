using Octo.Models.Domain;
using Octo.Services.LastFm;
using System.Diagnostics;

namespace Octo.Services.Common;

/// <summary>
/// Builds the external (discovery) half of a search, once per query.
///
/// Every caller for the same query joins one execution and receives the same list. That
/// is not an optimisation, it is what keeps the search3 fix from re-creating issue #8.
/// Clients routinely fire several search calls for one typed query, and registry ids are
/// deterministic, so those calls resolve to the *same* SoulseekRouting objects and would
/// each run the enrichment pipeline over them concurrently — three writers to a shared
/// int? duration, and three times the Deezer fan-out against a budget that is already the
/// tightest thing in the system.
///
/// The returned list is FROZEN. Nothing may mutate a Song after the build completes;
/// callers slice it and serialise it, concurrently, without copying. Both Subsonic
/// serialisers and the native one only read, and the star/download paths rebuild a Song
/// from the registry rather than from a search result. Any future "top up the enrichment
/// because this caller wanted more rows" belongs inside the build, not after it.
/// </summary>
public sealed class ExternalSearchService
{
    /// <summary>
    /// Rows built per query, regardless of how many the caller wants.
    ///
    /// It has to be a constant rather than the caller's target, or single-flight is
    /// unsound: a client asking for 8 rows could win the race and hand 8 rows to a caller
    /// that asked for 150. 60 is the number because that is where enrichment stops
    /// (BackgroundEnrichLimit), so rows past it would ship as bare placeholders carrying a
    /// fallback duration, and because the Navidrome-native search path already caps here.
    /// </summary>
    public const int BuildSize = 60;

    /// <summary>
    /// A normal Subsonic search page asks for 20 songs. Artist top-tracks is therefore
    /// only a fallback for a genuinely thin track.search response, not a mandatory
    /// second Last.fm request just to grow an already useful 50 rows to 60.
    /// </summary>
    private const int TopTracksFallbackThreshold = 20;

    /// <summary>
    /// One broad Deezer page is enough to cover the visible part of a normal search and
    /// still small enough to keep response parsing cheap. Exact artist/title lookups fill
    /// any gaps after Last.fm returns its authoritative candidate list.
    /// </summary>
    private const int MetadataPrefetchSize = 25;

    /// <summary>
    /// Deadline for one build. Last.fm has no configured HTTP timeout of its own, so
    /// without this a single hung call would pin the query for every joined caller.
    /// </summary>
    private static readonly TimeSpan BuildTimeout = TimeSpan.FromSeconds(10);

    private readonly SingleFlight<string, List<Song>> _flight = new();
    private readonly IMusicMetadataService _metadata;
    private readonly LastFmService? _lastFm;
    private readonly ILogger<ExternalSearchService> _logger;

    public ExternalSearchService(
        IMusicMetadataService metadata,
        ILogger<ExternalSearchService> logger,
        LastFmService? lastFm = null)
    {
        _metadata = metadata;
        _logger = logger;
        _lastFm = lastFm;
    }

    /// <summary>
    /// Up to <see cref="BuildSize"/> enriched external songs for this query. Callers take
    /// the prefix they need; the list is shared and must not be mutated.
    /// </summary>
    public async Task<IReadOnlyList<Song>> GetAsync(string query, CancellationToken ct = default)
    {
        if (string.IsNullOrWhiteSpace(query)) return Array.Empty<Song>();
        // Only the key, not the radio switch. Discovery in the search bar is a different
        // feature from radio, and gating it on EnableRadio made turning radio off empty
        // the search results too.
        if (_lastFm is null || !_lastFm.HasApiKey) return Array.Empty<Song>();

        // Key on the query alone. The build size is constant, so two callers wanting
        // different row counts still want the same work done.
        var key = query.Trim().ToLowerInvariant();

        try
        {
            return await _flight.RunAsync(key, token => BuildAsync(query, token), BuildTimeout);
        }
        catch (Exception ex)
        {
            // Discovery is an addition to search, never a precondition for it. The steps
            // inside the build are individually best-effort, but the deadline is not: the
            // enrichment fan-out waits on its semaphore outside its own try, so a timeout
            // there would otherwise escape and take the user's local results down with it.
            _logger.LogDebug("external search '{Q}' failed: {M}", query, ex.Message);
            return Array.Empty<Song>();
        }
    }

    /// <summary>
    /// Starts broad Deezer metadata prefetch alongside Last.fm, then fills in the metadata
    /// a client needs to render and play the rows. Candidate order:
    ///   1. track.search hits (best fuzzy matches for the query as typed)
    ///   2. canonical artist's top tracks (in case (1) was thin — common for
    ///      single-word artist queries)
    /// Deduped by artist+title so the same track cannot appear twice.
    /// </summary>
    private async Task<List<Song>> BuildAsync(string query, CancellationToken ct)
    {
        var totalWatch = Stopwatch.StartNew();
        var stageWatch = Stopwatch.StartNew();
        var stage = "lastfm";
        var outcome = "failed";
        long lastFmMs = -1;
        long placeholdersMs = -1;
        long deezerPrefetchMs = -1;
        long deezerMs = -1;
        long youtubeMs = -1;
        var deezerPrefetchRows = 0;
        var songCount = 0;

        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var collected = new List<(string Artist, string Title)>();
        try
        {
            // This broad Deezer lookup depends only on the raw query, so overlap it with
            // Last.fm instead of paying both network latencies serially. It primes the
            // exact per-track cache; rows it does not cover use the existing fallback.
            var deezerPrefetchTask = PrefetchSearchMetadataTimedAsync(query, ct);
            var tracks = await _lastFm!.SearchTracksAsync(query, Math.Min(50, BuildSize * 2));
            foreach (var t in tracks)
            {
                var key = $"{t.Artist}|{t.Title}".ToLowerInvariant();
                if (seen.Add(key)) collected.Add((t.Artist, t.Title));
                if (collected.Count >= BuildSize) break;
            }

            if (collected.Count < TopTracksFallbackThreshold)
            {
                // Use the first track-search hit's artist as the canonical anchor
                // for top-tracks fallback. Falls back to the raw query string when
                // track.search came back empty. Fill up to BuildSize once the fallback
                // is needed, but do not make this second HTTP request for a healthy page.
                var anchor = tracks.Count > 0 ? tracks[0].Artist : query;
                var topTracks = await _lastFm.GetArtistTopTracksAsync(anchor, BuildSize * 2);
                foreach (var t in topTracks)
                {
                    var key = $"{t.Artist}|{t.Title}".ToLowerInvariant();
                    if (seen.Add(key)) collected.Add((t.Artist, t.Title));
                    if (collected.Count >= BuildSize) break;
                }
            }

            lastFmMs = stageWatch.ElapsedMilliseconds;
            stage = "placeholders";
            stageWatch.Restart();

            var songs = new List<Song>(collected.Count);
            foreach (var (artist, title) in collected)
            {
                var hits = await _metadata.SearchSongsByArtistTitleAsync(artist, title, 1);
                if (hits.Count > 0) songs.Add(hits[0]);
            }
            songCount = songs.Count;
            placeholdersMs = stageWatch.ElapsedMilliseconds;
            _logger.LogInformation("External search '{Q}' -> {N} placeholder songs", query, songs.Count);

            // Album/art/duration from Deezer. Release year stays lazy because it would
            // add one extra Deezer request for every foreground row. Awaiting the broad
            // task here usually costs nothing because it has run alongside Last.fm.
            stage = "deezer";
            stageWatch.Restart();
            var prefetch = await deezerPrefetchTask;
            deezerPrefetchRows = prefetch.Rows;
            deezerPrefetchMs = prefetch.ElapsedMilliseconds;
            await _metadata.EnrichExternalSongsAsync(songs, ct);
            deezerMs = stageWatch.ElapsedMilliseconds;

            // YouTube is deliberately outside the response's critical path. The Deezer
            // duration is accurate enough to render search results; resolving the exact
            // YouTube upload synchronously imposed a ~2.3 s floor even on a healthy shim.
            // Prewarm updates routing only, so the frozen Song list remains safe to
            // serialize while video ids and stream URLs are prepared in the background.
            stage = "youtube-background";
            youtubeMs = 0;
            _ = _metadata.PrewarmYouTubeIdsAsync(songs, topN: 12);

            stage = "done";
            outcome = "completed";
            return songs;
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            outcome = "timeout";
            throw;
        }
        finally
        {
            var partialMs = stageWatch.ElapsedMilliseconds;
            switch (stage)
            {
                case "lastfm" when lastFmMs < 0: lastFmMs = partialMs; break;
                case "placeholders" when placeholdersMs < 0: placeholdersMs = partialMs; break;
                case "deezer" when deezerMs < 0: deezerMs = partialMs; break;
                case "youtube" when youtubeMs < 0: youtubeMs = partialMs; break;
            }

            _logger.LogInformation(
                "External search timing '{Q}': outcome={Outcome} stopped_at={Stage} lastfm_ms={LastFmMs} placeholders_ms={PlaceholdersMs} deezer_prefetch_ms={DeezerPrefetchMs} deezer_prefetch_rows={DeezerPrefetchRows} deezer_ms={DeezerMs} youtube_ms={YouTubeMs} youtube_mode=background total_ms={TotalMs} songs={Songs}",
                query,
                outcome,
                stage,
                lastFmMs,
                placeholdersMs,
                deezerPrefetchMs,
                deezerPrefetchRows,
                deezerMs,
                youtubeMs,
                totalWatch.ElapsedMilliseconds,
                songCount);
        }
    }

    private async Task<(int Rows, long ElapsedMilliseconds)> PrefetchSearchMetadataTimedAsync(
        string query,
        CancellationToken ct)
    {
        var watch = Stopwatch.StartNew();
        try
        {
            var hits = await _metadata.PrefetchSearchMetadataAsync(query, MetadataPrefetchSize, ct);
            return (hits, watch.ElapsedMilliseconds);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception ex)
        {
            // Prefetch is an optimisation, never a requirement for search. The exact
            // enrichment calls below remain the fallback when a provider cannot batch.
            _logger.LogDebug("search metadata prefetch '{Q}' failed: {M}", query, ex.Message);
            return (0, watch.ElapsedMilliseconds);
        }
    }
}
