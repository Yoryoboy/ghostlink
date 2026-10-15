use destiny_adapters::sqlite::SqliteStore;
use destiny_core::Timestamp;
use destiny_core::wishlist::{
    SourcedWishlistRoll, WishlistRepository, WishlistRoll, WishlistSourceRecord, parse_wishlist,
};

const VOLTRON: &str = "title:Voltron\n\
description:Community rolls.\n\
title:Machine Guns\n\
//notes:Shared PvE notes.|tags:pve\n\
dimwishlist:item=10&perks=1,2\n\
dimwishlist:item=10&perks=3\n\
dimwishlist:item=-69420&perks=9#notes:Any gun with perk 9.\n\
dimwishlist:item=20&perks=1\n\
dimwishlist:item=bad\n";

const LOCAL: &str = "dimwishlist:item=-10#notes:Mine says trash.\n";

fn roll(ordinal: u32, item: Option<u32>, perks: &[u32], notes: &str) -> WishlistRoll {
    WishlistRoll {
        ordinal,
        item_hash: item,
        trash: false,
        perks: perks.to_vec(),
        notes: Some(notes.into()),
        section: Some("Machine Guns".into()),
    }
}

fn publish(store: &mut SqliteStore, key: &str, text: &str, etag: Option<&str>, at: u64) {
    store
        .publish_wishlist(key, &parse_wishlist(text), etag, "fp-1", Timestamp::new(at))
        .unwrap();
}

#[test]
fn published_sources_round_trip_in_configured_order() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    assert!(store.wishlist_sources().unwrap().is_empty());
    assert!(store.wishlist_rolls(10).unwrap().is_empty());

    publish(&mut store, "voltron", VOLTRON, Some("\"etag-1\""), 100);
    publish(&mut store, "/lists/mine.txt", LOCAL, None, 100);
    store
        .retain_wishlist_sources(&["/lists/mine.txt".into(), "voltron".into()])
        .unwrap();

    assert_eq!(
        store.wishlist_sources().unwrap(),
        vec![
            WishlistSourceRecord {
                key: "/lists/mine.txt".into(),
                title: None,
                description: None,
                etag: None,
                fingerprint: "fp-1".into(),
                checked_at: Timestamp::new(100),
                updated_at: Timestamp::new(100),
                rolls: 1,
                skipped: 0,
            },
            WishlistSourceRecord {
                key: "voltron".into(),
                title: Some("Voltron".into()),
                description: Some("Community rolls.".into()),
                etag: Some("\"etag-1\"".into()),
                fingerprint: "fp-1".into(),
                checked_at: Timestamp::new(100),
                updated_at: Timestamp::new(100),
                rolls: 4,
                skipped: 1,
            },
        ]
    );
    let shared = "Shared PvE notes.|tags:pve";
    assert_eq!(
        store.wishlist_rolls(10).unwrap(),
        vec![
            SourcedWishlistRoll {
                source: "/lists/mine.txt".into(),
                roll: WishlistRoll {
                    ordinal: 0,
                    item_hash: Some(10),
                    trash: true,
                    perks: vec![],
                    notes: Some("Mine says trash.".into()),
                    section: None,
                },
            },
            SourcedWishlistRoll {
                source: "voltron".into(),
                roll: roll(0, Some(10), &[1, 2], shared),
            },
            SourcedWishlistRoll {
                source: "voltron".into(),
                roll: roll(1, Some(10), &[3], shared),
            },
            SourcedWishlistRoll {
                source: "voltron".into(),
                roll: roll(2, None, &[9], "Any gun with perk 9."),
            },
        ]
    );
    let other: Vec<_> = store
        .wishlist_rolls(20)
        .unwrap()
        .into_iter()
        .map(|sourced| (sourced.source, sourced.roll.ordinal))
        .collect();
    assert_eq!(other, vec![("voltron".into(), 2), ("voltron".into(), 3)]);
}

#[test]
fn republish_replaces_rolls_and_check_keeps_them() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    publish(&mut store, "voltron", VOLTRON, Some("\"etag-1\""), 100);
    store
        .mark_wishlist_checked("voltron", Some("\"etag-2\""), Timestamp::new(200))
        .unwrap();
    let checked = &store.wishlist_sources().unwrap()[0];
    assert_eq!(checked.etag.as_deref(), Some("\"etag-2\""));
    assert_eq!(checked.checked_at, Timestamp::new(200));
    assert_eq!(checked.updated_at, Timestamp::new(100));
    assert_eq!(checked.rolls, 4);

    publish(
        &mut store,
        "voltron",
        "dimwishlist:item=30&perks=4\n",
        None,
        300,
    );
    let replaced = &store.wishlist_sources().unwrap()[0];
    assert_eq!((replaced.rolls, replaced.skipped), (1, 0));
    assert_eq!(replaced.title, None);
    assert_eq!(replaced.etag, None);
    assert_eq!(replaced.updated_at, Timestamp::new(300));
    assert!(store.wishlist_rolls(10).unwrap().is_empty());
    assert_eq!(store.wishlist_rolls(30).unwrap().len(), 1);
}

#[test]
fn retain_drops_unlisted_sources_and_their_rolls() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    publish(&mut store, "voltron", VOLTRON, None, 100);
    publish(&mut store, "/lists/mine.txt", LOCAL, None, 100);
    store
        .retain_wishlist_sources(&["/lists/mine.txt".into()])
        .unwrap();
    let keys: Vec<_> = store
        .wishlist_sources()
        .unwrap()
        .into_iter()
        .map(|record| record.key)
        .collect();
    assert_eq!(keys, vec!["/lists/mine.txt".to_owned()]);
    assert_eq!(store.wishlist_rolls(20).unwrap(), vec![]);
    store.retain_wishlist_sources(&[]).unwrap();
    assert!(store.wishlist_sources().unwrap().is_empty());
    assert!(store.wishlist_rolls(10).unwrap().is_empty());
}

#[test]
fn check_of_an_unknown_source_is_a_storage_failure() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    assert!(
        store
            .mark_wishlist_checked("voltron", None, Timestamp::new(1))
            .is_err()
    );
}
