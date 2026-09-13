using System.Threading.Channels;

namespace Octo.Services.Listening;

public sealed class ListeningSubmissionWorker : BackgroundService, IListeningSubmissionQueue
{
    private static readonly TimeSpan PollInterval = TimeSpan.FromSeconds(30);
    private readonly IReadOnlyDictionary<string, IListeningSink> _sinks;
    private readonly PersistentScrobbleOutbox _outbox;
    private readonly ILogger<ListeningSubmissionWorker> _logger;
    private readonly Channel<NowPlayingWork> _nowPlaying = Channel.CreateBounded<NowPlayingWork>(
        new BoundedChannelOptions(100)
        {
            SingleReader = true,
            FullMode = BoundedChannelFullMode.DropOldest,
        });
    private readonly Channel<byte> _wake = Channel.CreateBounded<byte>(
        new BoundedChannelOptions(1)
        {
            SingleReader = true,
            FullMode = BoundedChannelFullMode.DropWrite,
        });

    public ListeningSubmissionWorker(
        IEnumerable<IListeningSink> sinks,
        PersistentScrobbleOutbox outbox,
        ILogger<ListeningSubmissionWorker> logger)
    {
        _sinks = sinks.ToDictionary(sink => sink.Name, StringComparer.OrdinalIgnoreCase);
        _outbox = outbox;
        _logger = logger;
    }

    public void QueueNowPlaying(ListeningSubmission submission)
    {
        var targets = EnabledSinkNames();
        if (targets.Length == 0) return;
        _nowPlaying.Writer.TryWrite(new NowPlayingWork(submission, targets));
        _wake.Writer.TryWrite(1);
    }

    public void QueueScrobble(ListeningSubmission submission)
    {
        var targets = EnabledSinkNames();
        if (targets.Length == 0) return;
        _outbox.Add(submission, targets, DateTimeOffset.UtcNow);
        _wake.Writer.TryWrite(1);
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation("Listening submission worker started");
        while (!stoppingToken.IsCancellationRequested)
        {
            await DrainNowPlayingAsync(stoppingToken);
            var processed = await ProcessDueScrobblesAsync(stoppingToken);
            if (processed >= 100)
            {
                _wake.Writer.TryWrite(1);
                continue;
            }

            using var wait = CancellationTokenSource.CreateLinkedTokenSource(stoppingToken);
            wait.CancelAfter(PollInterval);
            try
            {
                await _wake.Reader.WaitToReadAsync(wait.Token);
            }
            catch (OperationCanceledException) when (!stoppingToken.IsCancellationRequested)
            {
                // Periodic retry tick.
            }
            while (_wake.Reader.TryRead(out _)) { }
        }
        _logger.LogInformation("Listening submission worker stopped");
    }

    private async Task DrainNowPlayingAsync(CancellationToken cancellationToken)
    {
        while (_nowPlaying.Reader.TryRead(out var work))
        {
            foreach (var sinkName in work.TargetSinks)
            {
                if (!_sinks.TryGetValue(sinkName, out var sink) || !sink.IsEnabled) continue;
                try
                {
                    await sink.SendNowPlayingAsync(work.Submission, cancellationToken);
                }
                catch (Exception ex) when (ex is not OperationCanceledException)
                {
                    // Last.fm explicitly says Now Playing failures should not be retried.
                    _logger.LogWarning(
                        "{Sink} now-playing submission failed for {Artist} - {Title}: {Message}",
                        sink.Name, work.Submission.Track.Artist,
                        work.Submission.Track.Title, ex.Message);
                }
            }
        }
    }

    private async Task<int> ProcessDueScrobblesAsync(CancellationToken cancellationToken)
    {
        var now = DateTimeOffset.UtcNow;
        var due = _outbox.GetDue(now, 100);
        foreach (var pending in due)
        {
            var retryErrors = new List<string>();
            foreach (var sinkName in pending.PendingSinks)
            {
                if (!_sinks.TryGetValue(sinkName, out var sink) || !sink.IsEnabled)
                {
                    retryErrors.Add($"{sinkName}: destination is disabled or incomplete");
                    continue;
                }

                try
                {
                    await sink.SendScrobbleAsync(pending.Submission, cancellationToken);
                    _outbox.MarkDelivered(pending.Id, sinkName);
                    _logger.LogInformation(
                        "Scrobbled temporary track to {Sink}: {Artist} - {Title}",
                        sink.Name, pending.Submission.Track.Artist,
                        pending.Submission.Track.Title);
                }
                catch (ListeningSinkException ex) when (!ex.Retryable)
                {
                    // A malformed/filtered submission will never improve on retry. Drop
                    // only this destination so another sink may still accept the listen.
                    _outbox.MarkDelivered(pending.Id, sinkName);
                    _logger.LogWarning(
                        "Dropping permanent {Sink} scrobble failure for {Artist} - {Title}: {Message}",
                        sink.Name, pending.Submission.Track.Artist,
                        pending.Submission.Track.Title, ex.Message);
                }
                catch (Exception ex) when (ex is not OperationCanceledException)
                {
                    retryErrors.Add($"{sink.Name}: {ex.Message}");
                }
            }

            if (retryErrors.Count > 0)
            {
                var delay = RetryDelay(pending.Attempts);
                _outbox.Defer(pending.Id, string.Join(" | ", retryErrors), now + delay);
                _logger.LogWarning(
                    "Scrobble delivery deferred for {Delay}: {Errors}", delay, retryErrors);
            }
        }
        return due.Count;
    }

    private string[] EnabledSinkNames() => _sinks.Values
        .Where(sink => sink.IsEnabled)
        .Select(sink => sink.Name)
        .ToArray();

    private static TimeSpan RetryDelay(int previousAttempts)
    {
        var exponent = Math.Min(Math.Max(previousAttempts, 0), 8);
        return TimeSpan.FromSeconds(Math.Min(3600, 15 * Math.Pow(2, exponent)));
    }

    private sealed record NowPlayingWork(
        ListeningSubmission Submission,
        IReadOnlyList<string> TargetSinks);
}
