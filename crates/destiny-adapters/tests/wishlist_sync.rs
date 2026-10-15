use destiny_adapters::sqlite::SqliteStore;
use destiny_app::wishlist::{WishlistSourceReport, WishlistSyncReport, sync_wishlists};
use destiny_core::wishlist::{
    WishlistFetch, WishlistGateway, WishlistRepository, WishlistSource, WishlistSourceRecord,
};
use destiny_core::{AppError, ErrorCategory, Timestamp};
use std::collections::HashMap;
use std::path::PathBuf;

const MINE: &str = "/lists/mine.txt";

/// Serves scripted text per source; voltron honours its ETag like GitHub.
#[derive(Default)]
struct Gateway {
    texts: HashMap<String, String>,
    voltron_etag: Option<String>,
    failing: Vec<String>,
    fetches: Vec<(String, Option<String>)>,
}

impl WishlistGateway for Gateway {
    fn fetch(
        &mut self,
        source: &WishlistSource,
        etag: Option<&str>,
    ) -> Result<WishlistFetch, AppError> {
        let key = source.key();
        self.fetches.push((key.clone(), etag.map(str::to_owned)));
        if self.failing.contains(&key) {
            return Err(AppError::new(ErrorCategory::ProviderUnavailable));
        }
        let is_voltron = *source == WishlistSource::Voltron;
        if is_voltron && etag.is_some() && etag == self.voltron_etag.as_deref() {
            return Ok(WishlistFetch::NotModified);
        }
        Ok(WishlistFetch::Content {
            text: self.texts[&key].clone(),
            etag: is_voltron.then(|| self.voltron_etag.clone()).flatten(),
        })
    }
}

fn sources() -> Vec<WishlistSource> {
    vec![
        WishlistSource::Voltron,
        WishlistSource::Local(PathBuf::from(MINE)),
    ]
}

fn gateway() -> Gateway {
    Gateway {
        texts: HashMap::from([
            (
                "voltron".to_owned(),
                "title:Voltron\ndimwishlist:item=10&perks=1\ndimwishlist:item=bad\n".to_owned(),
            ),
            (MINE.to_owned(), "dimwishlist:item=-10\n".to_owned()),
        ]),
        voltron_etag: Some("\"v1\"".into()),
        ..Gateway::default()
    }
}

fn source(
    source: &str,
    status: &'static str,
    rolls: usize,
    skipped: usize,
) -> WishlistSourceReport {
    WishlistSourceReport {
        source: source.into(),
        status,
        rolls,
        skipped,
    }
}

fn records(store: &SqliteStore) -> Vec<WishlistSourceRecord> {
    store.wishlist_sources().unwrap()
}

#[test]
fn first_sync_publishes_every_source_in_configured_order() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut gateway = gateway();
    let report = sync_wishlists(&mut gateway, &mut store, &sources(), Timestamp::new(1));
    assert_eq!(
        report,
        WishlistSyncReport {
            status: "ok",
            sources: vec![
                source("voltron", "updated", 1, 1),
                source(MINE, "updated", 1, 0)
            ],
        }
    );
    assert_eq!(
        gateway.fetches,
        vec![("voltron".into(), None), (MINE.into(), None)]
    );
    let keys: Vec<_> = records(&store)
        .into_iter()
        .map(|record| record.key)
        .collect();
    assert_eq!(keys, vec!["voltron".to_owned(), MINE.to_owned()]);
    assert_eq!(records(&store)[0].etag.as_deref(), Some("\"v1\""));
    let sourced: Vec<_> = store
        .wishlist_rolls(10)
        .unwrap()
        .into_iter()
        .map(|sourced| (sourced.source, sourced.roll.trash))
        .collect();
    assert_eq!(
        sourced,
        vec![("voltron".into(), false), (MINE.into(), true)]
    );
}

#[test]
fn unchanged_sources_are_checked_without_republishing() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut gateway = gateway();
    sync_wishlists(&mut gateway, &mut store, &sources(), Timestamp::new(1));
    gateway.fetches.clear();
    let report = sync_wishlists(&mut gateway, &mut store, &sources(), Timestamp::new(2));
    assert_eq!(
        report.sources,
        vec![
            source("voltron", "unchanged", 1, 1),
            source(MINE, "unchanged", 1, 0)
        ]
    );
    assert_eq!(report.status, "ok");
    assert_eq!(
        gateway.fetches,
        vec![
            ("voltron".into(), Some("\"v1\"".into())),
            (MINE.into(), None)
        ]
    );
    for record in records(&store) {
        assert_eq!(record.checked_at, Timestamp::new(2), "{}", record.key);
        assert_eq!(record.updated_at, Timestamp::new(1), "{}", record.key);
    }

    gateway.voltron_etag = Some("\"v2\"".into());
    let report = sync_wishlists(&mut gateway, &mut store, &sources(), Timestamp::new(3));
    assert_eq!(report.sources[0], source("voltron", "unchanged", 1, 1));
    assert_eq!(records(&store)[0].etag.as_deref(), Some("\"v2\""));
    assert_eq!(records(&store)[0].updated_at, Timestamp::new(1));

    gateway
        .texts
        .insert(MINE.into(), "dimwishlist:item=20&perks=2\n".into());
    let report = sync_wishlists(&mut gateway, &mut store, &sources(), Timestamp::new(4));
    assert_eq!(report.sources[1], source(MINE, "updated", 1, 0));
    assert_eq!(store.wishlist_rolls(20).unwrap()[0].source, MINE);
}

#[test]
fn a_failing_source_keeps_its_rolls_and_only_degrades_the_status() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut gateway = gateway();
    sync_wishlists(&mut gateway, &mut store, &sources(), Timestamp::new(1));
    let before = records(&store);
    gateway.failing = vec!["voltron".into()];
    let report = sync_wishlists(&mut gateway, &mut store, &sources(), Timestamp::new(2));
    assert_eq!(
        report,
        WishlistSyncReport {
            status: "partial",
            sources: vec![
                source("voltron", "unavailable", 1, 1),
                source(MINE, "unchanged", 1, 0)
            ],
        }
    );
    assert_eq!(records(&store)[0], before[0]);
    gateway.failing = vec!["voltron".into(), MINE.into()];
    let report = sync_wishlists(&mut gateway, &mut store, &sources(), Timestamp::new(3));
    assert_eq!(report.status, "unavailable");
    assert_eq!(store.wishlist_rolls(10).unwrap().len(), 2);
}

#[test]
fn a_new_source_that_fails_reports_no_rolls() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut gateway = gateway();
    gateway.failing = vec![MINE.into()];
    let report = sync_wishlists(&mut gateway, &mut store, &sources(), Timestamp::new(1));
    assert_eq!(report.sources[1], source(MINE, "unavailable", 0, 0));
    assert_eq!(records(&store).len(), 1);
}

#[test]
fn removed_sources_are_dropped_and_none_disables_every_list() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut gateway = gateway();
    sync_wishlists(&mut gateway, &mut store, &sources(), Timestamp::new(1));
    let only_mine = [WishlistSource::Local(PathBuf::from(MINE))];
    let report = sync_wishlists(&mut gateway, &mut store, &only_mine, Timestamp::new(2));
    assert_eq!(report.sources, vec![source(MINE, "unchanged", 1, 0)]);
    let keys: Vec<_> = records(&store)
        .into_iter()
        .map(|record| record.key)
        .collect();
    assert_eq!(keys, vec![MINE.to_owned()]);

    gateway.fetches.clear();
    let report = sync_wishlists(&mut gateway, &mut store, &[], Timestamp::new(3));
    assert_eq!(
        report,
        WishlistSyncReport {
            status: "disabled",
            sources: vec![],
        }
    );
    assert!(gateway.fetches.is_empty());
    assert!(records(&store).is_empty());
    assert!(store.wishlist_rolls(10).unwrap().is_empty());
}

#[test]
fn reordering_sources_changes_the_stored_order_without_refetching_content() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut gateway = gateway();
    sync_wishlists(&mut gateway, &mut store, &sources(), Timestamp::new(1));
    let reversed: Vec<_> = sources().into_iter().rev().collect();
    let report = sync_wishlists(&mut gateway, &mut store, &reversed, Timestamp::new(2));
    assert_eq!(report.status, "ok");
    let keys: Vec<_> = records(&store)
        .into_iter()
        .map(|record| record.key)
        .collect();
    assert_eq!(keys, vec![MINE.to_owned(), "voltron".to_owned()]);
}

#[test]
fn invalid_configuration_leaves_stored_lists_untouched() {
    use destiny_app::wishlist::sync_wishlists_after;
    let mut store = SqliteStore::open_in_memory().unwrap();
    sync_wishlists(&mut gateway(), &mut store, &sources(), Timestamp::new(1));
    let before = records(&store);
    let mut acquired = false;
    let report = sync_wishlists_after(
        WishlistSource::parse_list(Some("relative/list.txt")),
        || {
            acquired = true;
            Ok(gateway())
        },
        &mut store,
        Timestamp::new(2),
    );
    assert_eq!(report, WishlistSyncReport::invalid_config());
    assert!(!acquired);
    assert_eq!(records(&store), before);
}

#[test]
fn without_a_gateway_every_source_reports_its_stored_rolls() {
    use destiny_app::wishlist::sync_wishlists_after;
    let mut store = SqliteStore::open_in_memory().unwrap();
    sync_wishlists(&mut gateway(), &mut store, &sources(), Timestamp::new(1));
    let before = records(&store);
    let report = sync_wishlists_after(
        Ok(sources()),
        || Err::<Gateway, _>(AppError::new(ErrorCategory::ProviderUnavailable)),
        &mut store,
        Timestamp::new(2),
    );
    assert_eq!(
        report,
        WishlistSyncReport {
            status: "unavailable",
            sources: vec![
                source("voltron", "unavailable", 1, 1),
                source(MINE, "unavailable", 1, 0)
            ],
        }
    );
    assert_eq!(records(&store), before);
}

#[test]
fn an_empty_voltron_body_never_replaces_stored_rolls() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut gateway = gateway();
    sync_wishlists(&mut gateway, &mut store, &sources(), Timestamp::new(1));
    let before = records(&store);
    gateway.voltron_etag = Some("\"truncated\"".into());
    gateway
        .texts
        .insert("voltron".into(), "<html>rate limited</html>\n".into());
    let report = sync_wishlists(&mut gateway, &mut store, &sources(), Timestamp::new(2));
    assert_eq!(
        report,
        WishlistSyncReport {
            status: "partial",
            sources: vec![
                source("voltron", "unavailable", 1, 1),
                source(MINE, "unchanged", 1, 0)
            ],
        }
    );
    assert_eq!(records(&store)[0], before[0]);
    assert_eq!(store.wishlist_rolls(10).unwrap().len(), 2);

    // A first sync must not store an empty voltron whose ETag would then
    // answer 304 until upstream changes.
    let mut fresh = SqliteStore::open_in_memory().unwrap();
    let report = sync_wishlists(
        &mut gateway,
        &mut fresh,
        &[WishlistSource::Voltron],
        Timestamp::new(3),
    );
    assert_eq!(report.sources, vec![source("voltron", "unavailable", 0, 0)]);
    assert!(records(&fresh).is_empty());
}

#[test]
fn an_emptied_local_list_is_published_as_the_user_left_it() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut gateway = gateway();
    sync_wishlists(&mut gateway, &mut store, &sources(), Timestamp::new(1));
    gateway.texts.insert(MINE.into(), String::new());
    let report = sync_wishlists(&mut gateway, &mut store, &sources(), Timestamp::new(2));
    assert_eq!(report.sources[1], source(MINE, "updated", 0, 0));
    assert_eq!(report.status, "ok");
    assert_eq!(records(&store)[1].rolls, 0);
}
