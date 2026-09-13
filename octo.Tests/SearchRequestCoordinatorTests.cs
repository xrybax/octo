using Octo.Services.Subsonic;

namespace Octo.Tests;

public sealed class SearchRequestCoordinatorTests
{
    [Fact]
    public void DefaultDebounceIsThreeHundredMilliseconds()
    {
        Assert.Equal(TimeSpan.FromMilliseconds(300), SearchRequestCoordinator.DefaultDebounce);
    }

    [Fact]
    public async Task NewerQuerySupersedesEarlierQueryWithoutWaitingForFullDebounce()
    {
        var release = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var coordinator = new SearchRequestCoordinator(
            TimeSpan.FromSeconds(30),
            (_, _) => release.Task);

        var earlier = coordinator.Begin("client", "ma");
        var waiting = coordinator.WaitForLatestAsync(earlier);

        coordinator.Begin("client", "madonna");

        Assert.False(await waiting.WaitAsync(TimeSpan.FromSeconds(1)));
        Assert.False(coordinator.IsLatest(earlier));
    }

    [Fact]
    public async Task SameNormalizedQuerySharesGenerationAcrossCategoryRequests()
    {
        var coordinator = new SearchRequestCoordinator(TimeSpan.Zero);

        var songs = coordinator.Begin("client", " Madonna ");
        var albums = coordinator.Begin("client", "madonna");

        Assert.Equal(songs.Generation, albums.Generation);
        Assert.True(await coordinator.WaitForLatestAsync(songs));
        Assert.True(await coordinator.WaitForLatestAsync(albums));
    }

    [Fact]
    public async Task DifferentClientsDoNotSupersedeEachOther()
    {
        var coordinator = new SearchRequestCoordinator(TimeSpan.Zero);

        var firstClient = coordinator.Begin("client-a", "madonna");
        coordinator.Begin("client-b", "queen");

        Assert.True(await coordinator.WaitForLatestAsync(firstClient));
    }
}
