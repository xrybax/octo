using System.Collections.Concurrent;

namespace Octo.Services.Subsonic;

/// <summary>
/// Debounces external discovery for Subsonic search-as-you-type clients.
///
/// Local Navidrome search still starts immediately. Expensive Last.fm, Deezer and
/// YouTube work is allowed only when this is still the newest query from the same
/// user/client/address after a short quiet period. Concurrent requests for the same
/// final query share one generation, so category-specific calls can still join the
/// same <see cref="Common.ExternalSearchService"/> single-flight build.
/// </summary>
public sealed class SearchRequestCoordinator
{
    public static readonly TimeSpan DefaultDebounce = TimeSpan.FromMilliseconds(300);

    private static readonly long RetentionMilliseconds =
        (long)TimeSpan.FromMinutes(30).TotalMilliseconds;

    private readonly ConcurrentDictionary<string, ClientState> _latest =
        new(StringComparer.Ordinal);
    private readonly TimeSpan _debounce;
    private readonly Func<TimeSpan, CancellationToken, Task> _delay;
    private long _nextGeneration;
    private int _beginCount;

    public SearchRequestCoordinator()
        : this(DefaultDebounce, static (delay, ct) => Task.Delay(delay, ct))
    {
    }

    internal SearchRequestCoordinator(
        TimeSpan debounce,
        Func<TimeSpan, CancellationToken, Task>? delay = null)
    {
        if (debounce < TimeSpan.Zero)
            throw new ArgumentOutOfRangeException(nameof(debounce));

        _debounce = debounce;
        _delay = delay ?? ((value, ct) => Task.Delay(value, ct));
    }

    /// <summary>
    /// Registers a query. Repeated calls for the same normalized query reuse its
    /// generation; a different query supersedes the previous generation immediately.
    /// </summary>
    internal SearchRequestLease Begin(string clientKey, string query)
    {
        clientKey = string.IsNullOrWhiteSpace(clientKey) ? "anonymous" : clientKey;
        var normalizedQuery = query.Trim().ToLowerInvariant();
        var now = Environment.TickCount64;

        while (true)
        {
            if (!_latest.TryGetValue(clientKey, out var current))
            {
                var created = NewState(normalizedQuery, now);
                if (_latest.TryAdd(clientKey, created))
                {
                    PruneOccasionally(now);
                    return Lease(clientKey, created);
                }

                continue;
            }

            if (string.Equals(current.Query, normalizedQuery, StringComparison.Ordinal))
            {
                var refreshed = current with { LastSeenMilliseconds = now };
                if (_latest.TryUpdate(clientKey, refreshed, current))
                {
                    PruneOccasionally(now);
                    return Lease(clientKey, refreshed);
                }

                continue;
            }

            var replacement = NewState(normalizedQuery, now);
            if (_latest.TryUpdate(clientKey, replacement, current))
            {
                current.Superseded.TrySetResult();
                PruneOccasionally(now);
                return Lease(clientKey, replacement);
            }
        }
    }

    /// <summary>
    /// Waits for the quiet period and reports whether this query is still current.
    /// Superseded requests wake immediately instead of occupying the full delay.
    /// </summary>
    internal async Task<bool> WaitForLatestAsync(
        SearchRequestLease lease,
        CancellationToken ct = default)
    {
        if (!IsLatest(lease)) return false;

        var delayTask = _delay(_debounce, ct);
        var completed = await Task.WhenAny(delayTask, lease.Superseded);
        if (ReferenceEquals(completed, lease.Superseded)) return false;

        // Observe cancellation/failure from the injected delay before consulting state.
        await delayTask;
        return IsLatest(lease);
    }

    internal bool IsLatest(SearchRequestLease lease)
    {
        return _latest.TryGetValue(lease.ClientKey, out var current)
            && current.Generation == lease.Generation;
    }

    private ClientState NewState(string query, long now)
    {
        return new ClientState(
            query,
            Interlocked.Increment(ref _nextGeneration),
            now,
            new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously));
    }

    private static SearchRequestLease Lease(string clientKey, ClientState state)
    {
        return new SearchRequestLease(
            clientKey,
            state.Generation,
            state.Superseded.Task);
    }

    private void PruneOccasionally(long now)
    {
        // Client keys contain user/app/address data and can vary over a long-running
        // server lifetime. Opportunistic pruning keeps the coordinator bounded without
        // adding a timer or hosted service to a 400 ms request-level concern.
        if ((Interlocked.Increment(ref _beginCount) & 0xff) != 0) return;

        var cutoff = now - RetentionMilliseconds;
        var collection = (ICollection<KeyValuePair<string, ClientState>>)_latest;
        foreach (var pair in _latest)
        {
            if (pair.Value.LastSeenMilliseconds > cutoff) continue;
            if (collection.Remove(pair)) pair.Value.Superseded.TrySetResult();
        }
    }

    private sealed record ClientState(
        string Query,
        long Generation,
        long LastSeenMilliseconds,
        TaskCompletionSource Superseded);
}

internal readonly record struct SearchRequestLease(
    string ClientKey,
    long Generation,
    Task Superseded);
