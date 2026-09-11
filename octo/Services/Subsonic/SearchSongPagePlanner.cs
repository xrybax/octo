namespace Octo.Services.Subsonic;

/// <summary>
/// Maps a Subsonic songOffset/songCount window onto Octo's stable search order:
/// a small local prefix, the cached external discovery set, then any remaining
/// Navidrome matches. Keeping this arithmetic in one place prevents page two from
/// repeating the external rows already returned on page one.
/// </summary>
internal static class SearchSongPagePlanner
{
    internal static SearchSongPagePlan Create(
        int songOffset,
        int songCount,
        int localPrefixCount,
        int externalCount,
        bool localTailMayExist)
    {
        var offset = Math.Max(0, songOffset);
        var count = Math.Max(0, songCount);
        var prefix = Math.Max(0, localPrefixCount);
        var external = Math.Max(0, externalCount);
        var end = Math.Min((long)int.MaxValue, (long)offset + count);

        var prefixStart = Math.Min(offset, prefix);
        var prefixEnd = Math.Min(end, prefix);
        var prefixTake = (int)Math.Max(0, prefixEnd - prefixStart);

        var externalGlobalStart = (long)prefix;
        var externalGlobalEnd = externalGlobalStart + external;
        var externalIntersectionStart = Math.Max(offset, externalGlobalStart);
        var externalIntersectionEnd = Math.Min(end, externalGlobalEnd);
        var externalSkip = (int)Math.Min(
            external,
            Math.Max(0, externalIntersectionStart - externalGlobalStart));
        var externalTake = (int)Math.Max(0, externalIntersectionEnd - externalIntersectionStart);

        var tailGlobalStart = Math.Max((long)offset, externalGlobalEnd);
        var tailTake = localTailMayExist
            ? (int)Math.Max(0, end - tailGlobalStart)
            : 0;
        var tailOffset = (int)Math.Min(
            int.MaxValue,
            prefix + Math.Max(0, tailGlobalStart - externalGlobalEnd));

        return new SearchSongPagePlan(
            LocalPrefixSkip: prefixStart,
            LocalPrefixTake: prefixTake,
            ExternalSkip: externalSkip,
            ExternalTake: externalTake,
            LocalTailOffset: tailOffset,
            LocalTailTake: tailTake);
    }
}

internal readonly record struct SearchSongPagePlan(
    int LocalPrefixSkip,
    int LocalPrefixTake,
    int ExternalSkip,
    int ExternalTake,
    int LocalTailOffset,
    int LocalTailTake);
