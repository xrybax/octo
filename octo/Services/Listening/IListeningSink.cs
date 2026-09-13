namespace Octo.Services.Listening;

public interface IListeningSink
{
    string Name { get; }
    bool IsEnabled { get; }
    Task SendNowPlayingAsync(ListeningSubmission submission, CancellationToken cancellationToken);
    Task SendScrobbleAsync(ListeningSubmission submission, CancellationToken cancellationToken);
}

public interface IListeningSubmissionQueue
{
    void QueueNowPlaying(ListeningSubmission submission);
    void QueueScrobble(ListeningSubmission submission);
}

public sealed class ListeningSinkException : Exception
{
    public ListeningSinkException(string message, bool retryable, Exception? innerException = null)
        : base(message, innerException)
    {
        Retryable = retryable;
    }

    public bool Retryable { get; }
}
