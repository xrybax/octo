use super::*;

const BEAR_FACE: &str = "Let's Get Lost (feat. Devon Baldwin) [Bear//Face Remix]";
const ORIGINAL: &str = "Let's Get Lost (feat. Devon Baldwin)";

#[test]
fn a_remix_by_someone_else_is_another_version() {
    let gap = version_gap(BEAR_FACE, "Let's Get Lost (Teklife remix)", Against::Recording)
        .expect("another remixer");
    assert!(gap.other_remixer);
    assert!(gap.added.is_empty() && gap.missing.is_empty());
    assert_eq!(gap.words(), "a remix by someone else");
}

#[test]
fn the_same_remix_written_otherwise_is_the_same_version() {
    for got in [
        "Let's Get Lost (Bear//Face remix)",
        "G\u{2010}Eazy - Let's Get Lost (Bear++Face remix)",
        "Let's Get Lost (feat. Devon Baldwin) (Bear//Face Remix)",
    ] {
        assert_eq!(version_gap(BEAR_FACE, got, Against::Recording), None, "{got}");
    }
}

#[test]
fn the_plain_song_is_not_the_remix_asked_for() {
    let gap = version_gap(BEAR_FACE, "Let\u{2019}s Get Lost", Against::Recording).expect("plain");
    assert_eq!(gap.missing, BTreeSet::from(["remix".to_string()]));
    assert_eq!(gap.words(), "not the remix asked for");
}

#[test]
fn a_remix_is_not_the_original_asked_for() {
    let gap =
        version_gap(ORIGINAL, "Let's Get Lost (Bear//Face remix)", Against::Recording).expect("a remix");
    assert_eq!(gap.added, BTreeSet::from(["remix".to_string()]));
}

#[test]
fn every_titled_variant_against_the_original_is_another_version() {
    for got in [
        "Heat Waves (Sped Up)",
        "Heat Waves (Slowed + Reverb)",
        "Heat Waves - Nightcore",
        "Heat Waves (Extended Mix)",
        "Heat Waves (Club Mix)",
        "Heat Waves (VIP)",
        "Heat Waves (Instrumental)",
        "Heat Waves (A Cappella)",
        "Heat Waves (Karaoke Version)",
        "Heat Waves (Live)",
        "Heat Waves (Acoustic)",
        "Heat Waves (Demo)",
        "Heat Waves (8D Audio)",
    ] {
        assert!(other_version("Heat Waves", got, Against::Recording), "{got}");
        assert!(other_version(got, "Heat Waves", Against::Title), "{got} asked");
    }
}

#[test]
fn a_database_title_may_leave_out_what_it_writes_in_the_disambiguation() {
    for asked in [
        "Creep (Live)",
        "Creep (Acoustic)",
        "Creep (Demo)",
        "Creep (Cover)",
    ] {
        assert!(!other_version(asked, "Creep", Against::Recording), "{asked}");
        assert!(
            other_version(asked, "Creep", Against::Title),
            "{asked} as a title"
        );
    }
    for asked in [
        "Creep (Remix)",
        "Creep (Sped Up)",
        "Creep (Instrumental)",
        "Creep (Extended Mix)",
    ] {
        assert!(other_version(asked, "Creep", Against::Recording), "{asked}");
    }
}

#[test]
fn neutral_markers_change_nothing() {
    for (asked, got) in [
        ("Teardrop", "Teardrop - Remastered 2011"),
        ("Teardrop (Album Version)", "Teardrop"),
        ("Strobe", "Strobe (Original Mix)"),
        ("Song (Explicit)", "Song"),
        (ORIGINAL, "Let's Get Lost"),
    ] {
        assert_eq!(version_gap(asked, got, Against::Title), None, "{asked} / {got}");
    }
}

#[test]
fn a_remix_that_does_not_say_who_made_it_is_not_held_against_one_that_does() {
    assert_eq!(
        version_gap("Song (Remix)", "Song (Skrillex Remix)", Against::Recording),
        None
    );
    assert_eq!(
        version_gap("Song (Skrillex Remix)", "Song (Remix)", Against::Recording),
        None
    );
}
