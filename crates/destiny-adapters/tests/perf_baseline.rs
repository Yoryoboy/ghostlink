//! Offline SQLite baseline; no production behavior or timing thresholds change.
//!
//! Run: cargo test -p destiny-adapters --release --test perf_baseline -- --ignored --nocapture --test-threads=1
//! One warm-up followed by nine samples on the same production-opened connection.
//! Timers include repository materialization, but exclude input cloning, assertions,
//! result destruction, catalog seeding, migrations and network/provider decoding.
//! The temporary database is retained (its path is printed) for reproducibility.

use destiny_adapters::sqlite::SqliteStore;
use destiny_app::test_support::QueuedProfileGateway;
use destiny_app::{ProfileSyncService, QueryService};
use destiny_core::domain::ItemSlot;
use destiny_core::ports::WorldDefinitionRepository;
use destiny_core::*;
use std::hint::black_box;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const VERSION: &str = "perf-baseline-v1";
const NOW: Timestamp = Timestamp::new(1_724_000_000);
const RUNS: usize = 9;
const ITEM_COUNT: usize = 600;
const STACK_COUNT: usize = 300;
const DEFINITION_COUNT: i64 = 20_000;

fn character_id(index: usize) -> CharacterId {
    CharacterId::new(format!("{}", 900_000_000_000_000_001u64 + index as u64))
}

fn instance_id(index: usize) -> ItemInstanceId {
    ItemInstanceId::new(format!("{}", 700_000_000_000_000_001u64 + index as u64))
}

fn catalog() -> DefinitionReplacementInput {
    let definitions = (1..=DEFINITION_COUNT)
        .map(|hash| {
            let (name, item_type, slot, category) = match hash {
                1..=8_000 => (
                    format!("Scout Rifle {hash:05}"),
                    "Scout Rifle",
                    Some(ItemSlot::Kinetic),
                    None,
                ),
                8_001..=14_000 => (
                    format!("Expedition Helmet {hash:05}"),
                    "Helmet",
                    Some(ItemSlot::Helmet),
                    None,
                ),
                14_001..=19_700 => (
                    format!("Rampage {hash:05}"),
                    "Perk",
                    None,
                    Some("frames".to_owned()),
                ),
                _ => (format!("Material {hash:05}"), "Material", None, None),
            };
            ItemDefinition {
                hash: ItemHash::new(hash),
                bucket_hash: None,
                name,
                item_type: Some(item_type.into()),
                tier: Some(
                    if hash % 20 == 0 {
                        "Exotic"
                    } else {
                        "Legendary"
                    }
                    .into(),
                ),
                element: None,
                slot,
                class_type: None,
                plug_category: category,
                tags: vec!["baseline".into(), format!("family-{}", hash % 12)],
                version: DefinitionVersion::new(VERSION),
                does_postmaster_pull_have_side_effects: None,
                is_instance_item: Some(hash <= 14_000),
                max_stack_size: Some(if hash > 19_700 { 999 } else { 1 }),
                stack_unique_label: None,
            }
        })
        .collect();
    DefinitionReplacementInput {
        definitions: ManifestDefinitions {
            version: DefinitionVersion::new(VERSION),
            definitions,
        },
        checked_at: NOW,
        source: SourceMarker::Manifest,
    }
}

fn table<T>(definitions: Vec<T>) -> WorldDefinitionTable<T> {
    WorldDefinitionTable {
        definitions,
        skipped_count: 0,
    }
}

fn world() -> WorldDefinitionsAcquisition {
    WorldDefinitionsAcquisition {
        version: DefinitionVersion::new(VERSION),
        item_descriptions: vec![],
        stats: table(
            [
                "Range",
                "Stability",
                "Handling",
                "Reload",
                "Health",
                "Class",
            ]
            .into_iter()
            .enumerate()
            .map(|(i, name)| StatDefinition::new(i as u32 + 1, name.into()).unwrap())
            .collect(),
        ),
        loadout_names: table(vec![]),
        milestones: table(vec![
            MilestoneDefinition::new(1, "Milestone".into(), "".into(), 1).unwrap(),
        ]),
        activities: table(vec![
            ActivityDefinition::new(1, "Activity".into(), "".into(), 1, 1).unwrap(),
        ]),
        modifiers: table(vec![
            ActivityModifierDefinition::new(1, "Modifier".into(), "".into()).unwrap(),
        ]),
        objectives: table(vec![ObjectiveDefinition::new(1, "".into(), 1).unwrap()]),
        activity_types: table(vec![
            ActivityTypeDefinition::new(1, "Type".into(), "".into()).unwrap(),
        ]),
    }
}

fn profile() -> InventorySnapshotInput {
    let items = (0..ITEM_COUNT)
        .map(|i| ItemInstance {
            instance_id: instance_id(i),
            item_hash: ItemHash::new(if i % 2 == 0 {
                1 + (i * 37 % 8_000) as i64
            } else {
                8_001 + (i * 31 % 6_000) as i64
            }),
            location: match i % 10 {
                0 => ItemLocation::Character(character_id(i % 3)),
                1 => ItemLocation::Postmaster(character_id(i % 3)),
                _ => ItemLocation::Vault,
            },
            quantity: 1,
            equipped: i % 100 == 0,
            locked: i % 7 == 0,
            power: Some(1_800 + (i % 20) as u16),
            perks: (0..4)
                .map(|p| PerkSummary {
                    perk_hash: PerkHash::new(50_000 + p),
                    active: true,
                    visible: true,
                })
                .collect(),
            plugs: (0..5)
                .map(|p| PlugSummary {
                    plug_hash: ItemHash::new(14_001 + ((i + p) % 100) as i64),
                    enabled: true,
                })
                .collect(),
            available_plugs: (0..5)
                .flat_map(|socket_index| {
                    (0..3).map(move |p| AvailablePlug {
                        socket_index,
                        plug_hash: ItemHash::new(14_001 + ((i + p as usize + 5) % 100) as i64),
                    })
                })
                .collect(),
            stats: (1..=6)
                .map(|stat_hash| StatSummary {
                    stat_hash,
                    value: ((i * 17 + stat_hash as usize * 13) % 101) as i32,
                })
                .collect(),
        })
        .collect();
    InventorySnapshotInput {
        membership_id: MembershipId::new("123"),
        membership_type: MembershipType::new(3),
        definition_version: DefinitionVersion::new(VERSION),
        checked_at: NOW,
        source: SourceMarker::Profile,
        profile_currencies_coverage: ProfileCurrenciesCoverage::NotObserved,
        characters: (0..3)
            .map(|i| Character {
                id: character_id(i),
                name: Some(format!("Guardian {i}")),
                class: Some(["Titan", "Hunter", "Warlock"][i].into()),
                race: None,
                gender: None,
                light: Some(1_810),
                date_last_played: None,
            })
            .collect(),
        items,
        stacks: (0..STACK_COUNT)
            .map(|i| {
                StackObservation::new(
                    ItemHash::new(19_701 + i as i64),
                    BucketHash::new(4_100),
                    1 + (i % 99) as i64,
                    if i % 4 == 0 {
                        StackContainer::Postmaster(character_id(i % 3))
                    } else {
                        StackContainer::ProfileInventory
                    },
                )
                .unwrap()
            })
            .collect(),
        plug_set_entries: vec![],
    }
}

fn loadouts() -> Vec<CharacterLoadouts> {
    (0..3)
        .map(|c| CharacterLoadouts {
            character_id: character_id(c),
            loadouts: (0..10)
                .map(|slot_index| Loadout {
                    slot_index,
                    name_hash: 10,
                    color_hash: 20,
                    icon_hash: 30,
                    items: (0..10)
                        .map(|i| LoadoutItem {
                            instance_id: instance_id(c * 200 + i),
                            plugs: (0..5)
                                .map(|socket_index| LoadoutPlug {
                                    socket_index,
                                    plug_hash: ItemHash::new(14_001 + i as i64),
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect()
}

fn definitions(text: Option<&str>) -> DefinitionSearchQuery {
    DefinitionSearchQuery {
        text: text.map(|text| SearchTerm::new(text).unwrap()),
        item_type: None,
        tier: None,
        tags: vec![],
        element: None,
        slot: None,
        class_type: None,
        limit: 50,
    }
}

fn instances() -> InstanceSearchQuery {
    InstanceSearchQuery {
        offset: 0,
        sort: None,
        location: None,
        equipped: None,
        include_available: false,
        item_hashes: ItemHashFilter::Any,
        perk_hash: None,
        perk_name: None,
        mod_name: None,
        archetype: None,
        character_id: None,
        definitions: None,
        min_power: None,
        stats: vec![],
        limit: 50,
    }
}

fn sample<T>(operation: impl FnOnce() -> T, check: impl FnOnce(&T)) -> Duration {
    let start = Instant::now();
    let result = black_box(operation());
    let elapsed = start.elapsed();
    check(&result);
    elapsed
}

fn median(label: &str, mut run: impl FnMut() -> Duration) {
    run(); // Warm-up is checked, but never enters the sample set.
    let mut samples: Vec<_> = (0..RUNS).map(|_| run()).collect();
    samples.sort_unstable();
    println!(
        "{label:<36} | {:>12.3}",
        samples[RUNS / 2].as_secs_f64() * 1_000.0
    );
}

#[test]
#[ignore = "large synthetic SQLite timing baseline; run explicitly in release mode"]
fn sqlite_read_and_publication_baseline() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir()
        .join(format!("ghostlink-perf-{}-{unique}", std::process::id()))
        .join("projection.db");
    let mut store = SqliteStore::open(&path).unwrap();
    store.publish_manifest(catalog(), world()).unwrap();
    let mut input = profile();
    let saved = loadouts();
    let published = store
        .replace_current_profile(input.clone(), None, Some(saved.clone()))
        .unwrap();
    assert_eq!(published.items.len(), ITEM_COUNT);
    assert_eq!(published.stacks.len(), STACK_COUNT);
    assert_eq!(published.characters.len(), 3);
    assert_eq!(store.saved_loadouts().unwrap(), Some(saved.clone()));
    // Reopen before measuring: seed-time connection caches are not required.
    drop(store);
    let store = SqliteStore::open(&path).unwrap();
    let mut app = QueryService::new(ProfileSyncService::new(QueuedProfileGateway::new(), store));
    println!("Database (retained): {}", path.display());
    println!("Dataset: 20000 definitions, 600 instances, 300 stacks, 3 characters, 30 loadouts");
    println!(
        "Warm-cache median: {RUNS} samples, 1 warm-up; repository calls except service inspection"
    );
    println!("measurement                          |    median_ms");
    println!("-------------------------------------|-------------");
    for (label, query, expected) in [
        (
            "definition name (late match)",
            definitions(Some("Scout Rifle 08000")),
            1,
        ),
        (
            "definition name (no match)",
            definitions(Some("absent-sentinel")),
            0,
        ),
    ] {
        median(label, || {
            sample(
                || app.repository().search_definitions(&query).unwrap(),
                |hits| assert_eq!(hits.len(), expected),
            )
        });
    }
    let mut text = instances();
    text.definitions = Some(Box::new(definitions(Some("Scout Rifle"))));
    median("owned text", || {
        sample(
            || app.repository().search_instances(&text).unwrap(),
            |hits| {
                assert_eq!(hits.len(), 50);
                assert!(hits.iter().all(|i| i.item_hash.value() <= 8_000));
            },
        )
    });
    let mut sorted = instances();
    sorted.sort = Some("Range:desc".parse().unwrap());
    sorted.limit = 5;
    median("owned stat sort (limit 5)", || {
        sample(
            || app.repository().search_instances(&sorted).unwrap(),
            |hits| {
                assert_eq!(hits.len(), 5);
                assert!(hits.iter().all(|i| i.stats[0].value == 100));
            },
        )
    });
    let mut perk = instances();
    perk.perk_name = Some(vec![SearchTerm::new("Rampage 14001").unwrap()]);
    median("owned perk name", || {
        sample(
            || app.repository().search_instances(&perk).unwrap(),
            |hits| {
                assert!(!hits.is_empty());
                assert!(
                    hits.iter()
                        .all(|i| { i.plugs.iter().any(|p| p.plug_hash == ItemHash::new(14_001)) })
                );
            },
        )
    });
    let inspect = InspectItemRequest {
        instance_id: instance_id(0),
    };
    median("inspect item (service)", || {
        sample(
            || {
                app.inspect_item(&inspect, FreshnessPolicy { ttl_seconds: 300 }, NOW)
                    .unwrap()
            },
            |result| {
                let Resolution::Unique(item) = result else {
                    panic!("expected inspection")
                };
                assert_eq!(item.instance.instance_id, instance_id(0));
                assert_eq!(item.stats.len(), 6);
            },
        )
    });
    median("full committed snapshot", || {
        sample(
            || app.repository().load_current().unwrap().unwrap(),
            |snapshot| assert_eq!(snapshot, &published),
        )
    });
    let mut revision = published.metadata.revision;
    for changed in [false, true] {
        let label = if changed {
            "publish one changed item"
        } else {
            "publish unchanged"
        };
        median(label, || {
            input.checked_at = Timestamp::new(input.checked_at.seconds() + 1);
            if changed {
                input.items[0].locked = !input.items[0].locked;
            }
            // Prepare owned arguments before starting the timer. Every changed
            // sample (including warm-up) differs from the previous publication.
            let next = input.clone();
            let next_saved = saved.clone();
            sample(
                || {
                    app.repository_mut()
                        .replace_current_profile(next, None, Some(next_saved))
                        .unwrap()
                },
                |snapshot| {
                    assert_eq!(
                        snapshot.metadata.revision.value(),
                        revision.value() + u64::from(changed)
                    );
                    assert_eq!(snapshot.metadata.checked_at, input.checked_at);
                    assert_eq!(snapshot.items[0].locked, input.items[0].locked);
                    revision = snapshot.metadata.revision;
                },
            )
        });
    }
    assert_eq!(app.repository().saved_loadouts().unwrap(), Some(saved));
    let typed = search_query::compile("is:locked or is:invault").unwrap();
    let execution = search_query::ResolvedQuery::new(&typed);
    let stat_definitions = world().stats.definitions;
    let mut expected: Vec<_> = input
        .items
        .iter()
        .filter(|item| item.locked || item.location == ItemLocation::Vault)
        .cloned()
        .collect();
    expected
        .sort_by(|a, b| query::compare_instances(sorted.sort.as_ref(), a, b, &stat_definitions));
    expected.truncate(sorted.limit);
    median("owned residual OR (stat sort 5)", || {
        sample(
            || {
                app.repository()
                    .search_instance_query_page(&sorted, Some(&execution))
                    .unwrap()
            },
            |(hits, more)| {
                assert_eq!(hits, &expected);
                assert!(*more);
            },
        )
    });
}
