use destiny_core::wishlist::{
    PlugOption, WishlistSource, WishlistVerdict, evaluate_wishlist, notes_tags, notes_text,
    parse_wishlist, wishlist_fingerprint,
};
use destiny_core::{AppError, ErrorCategory};
use std::path::PathBuf;

const VOLTRON_SAMPLE: &str = "title:This is a compiled collection of god/recommended rolls.\r
description:Thanks to the curators.\r
\r
// taken from https://github.com/Adamsdown-Boy/destiny-rolls\r
\r
title:PvE Podcast 174 - The Best Machine Guns\r
description:Monument of Triumph machine guns.\r
\r
// Archon's Thunder - PvE Minor god 1\r
//notes:First-choice PvE roll.|tags:PvE PvE-God M+KB Controller\r
dimwishlist:item=91672792&perks=839105230,106909392,1955165503,124408337\r
dimwishlist:item=91672792&perks=3661387068,106909392,1955165503,124408337#notes:Line notes win.\r
dimwishlist:item=91672792&perks=1,2\r
\r
dimwishlist:item=91672792&perks=5\r
";

#[test]
fn parses_sections_block_notes_and_line_notes_in_file_order() {
    let list = parse_wishlist(VOLTRON_SAMPLE);

    assert_eq!(
        list.title.as_deref(),
        Some("This is a compiled collection of god/recommended rolls.")
    );
    assert_eq!(list.description.as_deref(), Some("Thanks to the curators."));
    assert_eq!(list.skipped, 0);
    assert_eq!(list.rolls.len(), 4);

    let first = &list.rolls[0];
    assert_eq!(first.ordinal, 0);
    assert_eq!(first.item_hash, Some(91672792));
    assert!(!first.trash);
    assert_eq!(
        first.perks,
        vec![839105230, 106909392, 1955165503, 124408337]
    );
    assert_eq!(
        first.section.as_deref(),
        Some("PvE Podcast 174 - The Best Machine Guns")
    );
    assert_eq!(
        first.notes.as_deref(),
        Some("First-choice PvE roll.|tags:PvE PvE-God M+KB Controller")
    );
    assert_eq!(list.rolls[1].notes.as_deref(), Some("Line notes win."));
    assert_eq!(list.rolls[2].notes, list.rolls[0].notes);
    // A blank line closes block notes; the section stays open.
    assert_eq!(list.rolls[3].ordinal, 3);
    assert_eq!(list.rolls[3].notes, None);
    assert_eq!(list.rolls[3].section, list.rolls[0].section);
}

#[test]
fn parses_trash_wildcard_bare_items_and_at_title_sections() {
    let list = parse_wishlist(
        "@title:Heresy additional rolls\n\
         dimwishlist:item=-3222518097\n\
         dimwishlist:item=-2824241403#notes:Duke is better\n\
         dimwishlist:item=-69420&perks=3400784728\n\
         dimwishlist:item=1234\n\
         dimwishlist:item=-3100452337#notes\n",
    );

    assert_eq!(list.skipped, 0);
    assert_eq!(list.title, None);
    let rolls = &list.rolls;
    assert_eq!(rolls[0].item_hash, Some(3222518097));
    assert!(rolls[0].trash);
    assert!(rolls[0].perks.is_empty());
    assert_eq!(rolls[0].section.as_deref(), Some("Heresy additional rolls"));
    assert_eq!(rolls[1].notes.as_deref(), Some("Duke is better"));
    assert!(rolls[1].trash);
    assert_eq!(rolls[2].item_hash, None, "-69420 is the any-item wildcard");
    assert!(!rolls[2].trash, "the wildcard is never trash");
    assert_eq!(rolls[2].perks, vec![3400784728]);
    assert_eq!(rolls[3].item_hash, Some(1234));
    assert!(rolls[3].perks.is_empty());
    assert!(rolls[4].trash, "a bare #notes marker is accepted");
    assert_eq!(rolls[4].notes, None);
}

#[test]
fn skips_and_counts_invalid_roll_lines_without_editing_them() {
    let list = parse_wishlist(
        "dimwishlist:item=abc&perks=1\n\
         dimwishlist:item=4294967296&perks=1\n\
         dimwishlist:item=1&perks=1,,2\n\
         dimwishlist:item=1&perks=99999999999\n\
         dimwishlist:item=-69420\n\
         dimwishlist:item=7&perks=8\n",
    );

    assert_eq!(list.skipped, 5);
    assert_eq!(list.rolls.len(), 1);
    assert_eq!(list.rolls[0].item_hash, Some(7));
    assert_eq!(list.rolls[0].ordinal, 0);
}

#[test]
fn unrecognized_lines_and_comments_close_block_notes() {
    let list = parse_wishlist(
        "//notes:Block A\n\
         dimwishlist:item=1&perks=2\n\
         // a comment\n\
         dimwishlist:item=1&perks=3\n\
         //notes:Block B\n\
         //notes:Block C\n\
         dimwishlist:item=1&perks=4\n",
    );

    assert_eq!(list.rolls[0].notes.as_deref(), Some("Block A"));
    assert_eq!(list.rolls[1].notes, None);
    assert_eq!(list.rolls[2].notes.as_deref(), Some("Block C"));
}

#[test]
fn oversized_lines_are_skipped_whole() {
    let long = format!("dimwishlist:item=1&perks=2#notes:{}\n", "x".repeat(20_000));
    let list = parse_wishlist(&long);

    assert_eq!(list.skipped, 1);
    assert!(list.rolls.is_empty());
}

#[test]
fn notes_split_text_and_normalized_tags() {
    assert_eq!(
        notes_text("First-choice PvE roll.|tags:PvE PvE-God M+KB Controller"),
        "First-choice PvE roll."
    );
    assert_eq!(
        notes_tags("First-choice PvE roll.|tags:PvE PvE-God M+KB Controller"),
        vec!["pve", "pve-god", "mkb", "controller"]
    );
    assert_eq!(
        notes_tags("x (tags: controller,god-pvp)|tags:controller,god-pvp,pvp"),
        vec!["controller", "pvp-god", "pvp"]
    );
    assert_eq!(
        notes_tags("tags:pve,mnk,god-pve,pve"),
        vec!["pve", "mkb", "pve-god"]
    );
    assert_eq!(notes_text("No tags here"), "No tags here");
    assert!(notes_tags("No tags here").is_empty());
}

fn options() -> Vec<PlugOption> {
    vec![
        PlugOption::new(839105230, "Arrowhead Brake"),
        PlugOption::new(106909392, "Tactical Mag"),
        // Enhanced Rimestealer: a different hash with the base name.
        PlugOption::new(900, "Rimestealer"),
        PlugOption::new(124408337, "Headstone"),
    ]
}

fn names(hash: u32) -> Option<String> {
    match hash {
        1955165503 => Some("Rimestealer".into()),
        _ => None,
    }
}

#[test]
fn every_wish_perk_must_be_offered_and_enhanced_options_match_by_name() {
    let list = parse_wishlist(VOLTRON_SAMPLE);
    let result = evaluate_wishlist(91672792, list.rolls.iter(), &options(), names);

    assert_eq!(result.verdict, WishlistVerdict::Wishlist);
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].ordinal, 0);

    let unknown = evaluate_wishlist(91672792, list.rolls.iter(), &options(), |_| None);
    assert_eq!(unknown.verdict, WishlistVerdict::None);
    assert!(unknown.matches.is_empty());
}

/// An unnamed option (no definition) matches only its exact hash: two blank
/// names are not the same perk.
#[test]
fn unnamed_options_never_match_by_name() {
    let list = parse_wishlist("dimwishlist:item=10&perks=9\n");
    let unnamed = [PlugOption::new(5, "")];
    let result = evaluate_wishlist(10, list.rolls.iter(), &unnamed, |_| Some(String::new()));
    assert_eq!(result.verdict, WishlistVerdict::None);

    let exact = [PlugOption::new(9, "")];
    let result = evaluate_wishlist(10, list.rolls.iter(), &exact, |_| None);
    assert_eq!(result.verdict, WishlistVerdict::Wishlist);
}

#[test]
fn rolls_for_other_items_never_match() {
    let list = parse_wishlist(VOLTRON_SAMPLE);
    let result = evaluate_wishlist(1, list.rolls.iter(), &options(), names);

    assert_eq!(result.verdict, WishlistVerdict::None);
    assert_eq!(result.total, 0);
}

#[test]
fn first_match_decides_the_verdict_and_trash_needs_no_perks() {
    let list = parse_wishlist(
        "//notes:god roll\n\
         dimwishlist:item=10&perks=839105230\n\
         \n\
         dimwishlist:item=-10#notes:archetype is bad\n",
    );
    let wished = evaluate_wishlist(10, list.rolls.iter(), &options(), names);
    assert_eq!(wished.verdict, WishlistVerdict::Wishlist);
    assert_eq!(wished.total, 2);
    assert!(wished.matches[1].trash);

    let other = [PlugOption::new(5, "Other")];
    let trashed = evaluate_wishlist(10, list.rolls.iter(), &other, names);
    assert_eq!(trashed.verdict, WishlistVerdict::Trash);
    assert_eq!(trashed.matches.len(), 1);
}

#[test]
fn wildcard_rolls_apply_to_any_item() {
    let list = parse_wishlist("dimwishlist:item=-69420&perks=124408337\n");
    let result = evaluate_wishlist(777, list.rolls.iter(), &options(), names);

    assert_eq!(result.verdict, WishlistVerdict::Wishlist);
}

#[test]
fn matches_are_grouped_by_section_and_notes_and_bounded_to_three() {
    let mut text = String::new();
    for group in 0..5 {
        text.push_str(&format!("//notes:group {group}\n"));
        text.push_str("dimwishlist:item=10&perks=839105230\n");
        text.push_str("dimwishlist:item=10&perks=106909392\n\n");
    }
    let list = parse_wishlist(&text);
    let result = evaluate_wishlist(10, list.rolls.iter(), &options(), names);

    assert_eq!(result.total, 5, "distinct section/notes groups");
    assert_eq!(result.matches.len(), 3);
    assert_eq!(
        result
            .matches
            .iter()
            .map(|m| m.notes.as_deref().unwrap())
            .collect::<Vec<_>>(),
        vec!["group 0", "group 1", "group 2"]
    );
}

/// `tags:` only opens the tag list at the start, after `|` or after
/// whitespace; inside a word (`hashtags:`) it is ordinary note text.
#[test]
fn tags_inside_a_word_are_note_text() {
    let notes = "Ignore the hashtags: they are jokes";
    assert_eq!(notes_text(notes), notes);
    assert!(notes_tags(notes).is_empty());
    let mixed = "See hashtags: below|tags:pve";
    assert_eq!(notes_text(mixed), "See hashtags: below");
    assert_eq!(notes_tags(mixed), vec!["pve"]);
}

#[test]
fn notes_text_and_tags_split_at_the_same_bare_tags_marker() {
    // Voltron has 442 lines whose tag list has no `|` before `tags:`.
    let notes = "Chevy's S18 Super Pog PVE List - MW Reload tags:pve, M+KB";
    assert_eq!(
        notes_text(notes),
        "Chevy's S18 Super Pog PVE List - MW Reload"
    );
    assert_eq!(notes_tags(notes), vec!["pve", "mkb"]);
    assert_eq!(notes_text("foo tags:pve"), "foo");
    assert_eq!(notes_text("tags:pve"), "");
    assert_eq!(
        notes_text("x (tags: controller,god-pvp)|tags:controller,god-pvp,pvp"),
        "x (tags: controller,god-pvp)"
    );
}

fn configuration_error(value: &str) -> AppError {
    WishlistSource::parse_list(Some(value)).unwrap_err()
}

#[test]
fn source_list_defaults_to_voltron_and_accepts_local_absolute_paths() {
    assert_eq!(
        WishlistSource::parse_list(None).unwrap(),
        vec![WishlistSource::Voltron]
    );
    assert_eq!(
        WishlistSource::parse_list(Some("  ")).unwrap(),
        vec![WishlistSource::Voltron]
    );
    assert_eq!(WishlistSource::parse_list(Some(" none ")).unwrap(), vec![]);
    let local = if cfg!(windows) {
        r"C:\lists\mine.txt"
    } else {
        "/lists/mine.txt"
    };
    let sources = WishlistSource::parse_list(Some(&format!("{local} , voltron"))).unwrap();
    assert_eq!(
        sources,
        vec![
            WishlistSource::Local(PathBuf::from(local)),
            WishlistSource::Voltron
        ]
    );
    assert_eq!(sources[0].key(), local);
    assert_eq!(sources[1].key(), "voltron");
}

#[test]
fn source_list_rejects_relative_empty_duplicate_and_mixed_none_entries() {
    for value in [
        "lists/mine.txt",
        "voltron,,voltron",
        "voltron,",
        "voltron,voltron",
        "none,voltron",
        "https://example.test/list.txt",
        "choosy_voltron",
    ] {
        assert_eq!(
            configuration_error(value).category(),
            ErrorCategory::ConfigurationError,
            "{value}"
        );
    }
}

#[test]
fn fingerprint_is_stable_and_detects_changes() {
    assert_eq!(wishlist_fingerprint(b""), "fnv1a64:cbf29ce484222325:0");
    assert_eq!(wishlist_fingerprint(b"a"), "fnv1a64:af63dc4c8601ec8c:1");
    assert_ne!(wishlist_fingerprint(b"ab"), wishlist_fingerprint(b"ba"));
}

#[test]
fn rolls_share_one_allocation_per_block_note_and_section() {
    // Voltron repeats 4.8 MB of distinct notes over 274,677 rolls; copying
    // them per roll cost about 204 MB while parsing.
    let document = parse_wishlist(
        "title:Guns\n//notes:Shared.|tags:pve\ndimwishlist:item=1&perks=2\ndimwishlist:item=1&perks=3\n",
    );
    let [first, second] = &document.rolls[..] else {
        panic!("expected two rolls");
    };
    assert_eq!(first.notes.as_deref(), Some("Shared.|tags:pve"));
    assert!(std::sync::Arc::ptr_eq(
        first.notes.as_ref().unwrap(),
        second.notes.as_ref().unwrap()
    ));
    assert!(std::sync::Arc::ptr_eq(
        first.section.as_ref().unwrap(),
        second.section.as_ref().unwrap()
    ));
}

mod inspection {
    use destiny_core::domain::ItemSlot;
    use destiny_core::wishlist::{
        PlugOption, SourcedWishlistRoll, WishlistInspection, WishlistMatch, WishlistVerdict,
        inspect_wishlist, parse_wishlist,
    };

    fn sourced(source: &str, text: &str) -> Vec<SourcedWishlistRoll> {
        parse_wishlist(text)
            .rolls
            .into_iter()
            .map(|roll| SourcedWishlistRoll {
                source: source.into(),
                roll,
            })
            .collect()
    }

    fn perk_name(hash: u32) -> Option<String> {
        match hash {
            1 => Some("Outlaw".into()),
            2 => Some("Rampage".into()),
            3 => Some("Kill Clip".into()),
            _ => None,
        }
    }

    fn rolls() -> Vec<SourcedWishlistRoll> {
        let mut rolls = sourced(
            "/mine.txt",
            "dimwishlist:item=10&perks=1,9#notes:mine|tags:pvp\n",
        );
        rolls.extend(sourced(
            "voltron",
            "title:Guns\n//notes:Great. tags:god-pve, M+KB\n\
             dimwishlist:item=10&perks=1,2\ndimwishlist:item=10&perks=1,3\n\
             dimwishlist:item=10&perks=4\n",
        ));
        rolls
    }

    #[test]
    fn weapons_cite_source_section_perk_names_notes_and_tags() {
        let options = [
            PlugOption::new(1, "Outlaw"),
            PlugOption::new(2, "Rampage"),
            PlugOption::new(3, "Kill Clip"),
            PlugOption::new(9, "Unnamed"),
        ];
        let inspection =
            inspect_wishlist(Some(ItemSlot::Kinetic), 10, &rolls(), &options, perk_name);
        assert_eq!(
            inspection,
            Some(WishlistInspection {
                verdict: WishlistVerdict::Wishlist,
                matches: vec![
                    WishlistMatch {
                        source: "/mine.txt".into(),
                        section: None,
                        perks: vec!["Outlaw".into(), "9".into()],
                        notes: Some("mine".into()),
                        tags: vec!["pvp".into()],
                    },
                    WishlistMatch {
                        source: "voltron".into(),
                        section: Some("Guns".into()),
                        perks: vec!["Outlaw".into(), "Rampage".into()],
                        notes: Some("Great.".into()),
                        tags: vec!["pve-god".into(), "mkb".into()],
                    },
                ],
                matches_total: 2,
            })
        );
    }

    /// Two lists that share a section and notes are still two opinions, each
    /// cited with its own source.
    #[test]
    fn lists_sharing_section_and_notes_are_cited_separately() {
        let text = "title:Guns\ndimwishlist:item=10&perks=1#notes:Great.\n";
        let mut rolls = sourced("/mine.txt", text);
        rolls.extend(sourced("voltron", text));
        let options = [PlugOption::new(1, "Outlaw")];
        let inspection = inspect_wishlist(Some(ItemSlot::Kinetic), 10, &rolls, &options, perk_name)
            .expect("weapon");
        let sources: Vec<&str> = inspection
            .matches
            .iter()
            .map(|found| found.source.as_str())
            .collect();
        assert_eq!(sources, ["/mine.txt", "voltron"]);
        assert_eq!(inspection.matches_total, 2);
    }

    #[test]
    fn unmatched_weapons_report_no_opinion_and_non_weapons_nothing() {
        assert_eq!(
            inspect_wishlist(Some(ItemSlot::Energy), 10, &rolls(), &[], perk_name),
            Some(WishlistInspection {
                verdict: WishlistVerdict::None,
                matches: vec![],
                matches_total: 0,
            })
        );
        for slot in [Some(ItemSlot::Chest), None] {
            assert_eq!(
                inspect_wishlist(slot, 10, &rolls(), &[], perk_name),
                None,
                "{slot:?}"
            );
        }
    }
}
