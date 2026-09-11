using Octo.Services.Subsonic;

namespace Octo.Tests;

public sealed class SearchSongPagePlannerTests
{
    [Fact]
    public void FirstTwenty_UsesLocalPrefixThenExternalRows()
    {
        var page = SearchSongPagePlanner.Create(
            songOffset: 0,
            songCount: 20,
            localPrefixCount: 12,
            externalCount: 50,
            localTailMayExist: true);

        Assert.Equal(new SearchSongPagePlan(0, 12, 0, 8, 12, 0), page);
    }

    [Theory]
    [InlineData(20, 8)]
    [InlineData(40, 28)]
    public void ContinuationPages_DoNotRepeatExternalRows(int offset, int expectedExternalSkip)
    {
        var page = SearchSongPagePlanner.Create(
            songOffset: offset,
            songCount: 20,
            localPrefixCount: 12,
            externalCount: 50,
            localTailMayExist: true);

        Assert.Equal(0, page.LocalPrefixTake);
        Assert.Equal(expectedExternalSkip, page.ExternalSkip);
        Assert.Equal(20, page.ExternalTake);
        Assert.Equal(0, page.LocalTailTake);
    }

    [Fact]
    public void PageCrossingEndOfDiscovery_AppendsRemainingLocalRowsAfterExternals()
    {
        var page = SearchSongPagePlanner.Create(
            songOffset: 60,
            songCount: 20,
            localPrefixCount: 12,
            externalCount: 50,
            localTailMayExist: true);

        Assert.Equal(48, page.ExternalSkip);
        Assert.Equal(2, page.ExternalTake);
        Assert.Equal(12, page.LocalTailOffset);
        Assert.Equal(18, page.LocalTailTake);
    }

    [Fact]
    public void PageAfterDiscovery_ContinuesAtTheCorrectNavidromeOffset()
    {
        var page = SearchSongPagePlanner.Create(
            songOffset: 80,
            songCount: 20,
            localPrefixCount: 12,
            externalCount: 50,
            localTailMayExist: true);

        Assert.Equal(50, page.ExternalSkip);
        Assert.Equal(0, page.ExternalTake);
        Assert.Equal(30, page.LocalTailOffset);
        Assert.Equal(20, page.LocalTailTake);
    }

    [Fact]
    public void ShortLocalPrefix_MeansThereIsNoLocalTail()
    {
        var page = SearchSongPagePlanner.Create(
            songOffset: 40,
            songCount: 20,
            localPrefixCount: 5,
            externalCount: 50,
            localTailMayExist: false);

        Assert.Equal(35, page.ExternalSkip);
        Assert.Equal(15, page.ExternalTake);
        Assert.Equal(0, page.LocalTailTake);
    }

    [Fact]
    public void NegativeInputs_AreTreatedAsZero()
    {
        var page = SearchSongPagePlanner.Create(-20, -1, -12, -50, false);

        Assert.Equal(new SearchSongPagePlan(0, 0, 0, 0, 0, 0), page);
    }
}
