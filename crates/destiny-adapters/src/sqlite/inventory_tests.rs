//! Focused contract tests for the transactional SQLite inventory repository.
//!
//! Every test is offline, credential-free, and deterministic. File-backed tests
//! use a private temporary path derived from the process id and an atomic
//! counter, and always remove the database and its sidecars.

use super::SqliteStore;
use destiny_core::{
    AppError, Character, DefinitionReplacementInput, DefinitionRepository, DefinitionVersion,
    ErrorCategory, InventoryRepository, InventorySnapshotInput, ItemDefinition, ItemHash,
    ItemInstance, ItemLocation, ManifestDefinitions, MembershipId, MembershipType, PerkHash,
    PerkSummary, Resolution, SourceMarker, Timestamp,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

/// A private temporary database path plus cleanup of its SQLite sidecars.
struct TempDatabase {
    path: PathBuf,
}

impl TempDatabase {
    fn new() -> Self {
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "destiny-inventory-{}-{sequence}.db",
            std::process::id()
        ));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut sidecar = self.path.clone().into_os_string();
            sidecar.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(sidecar));
        }
    }
}

fn storage_error() -> AppError {
    AppError::new(ErrorCategory::LocalStorage)
}

/// A complete projection covering every optional field, all three typed
/// locations, the exact numeric bounds of the schema columns, and an ordered
/// perk list that includes a duplicated hash, both state flags, both `u32` hash
/// bounds, and an empty list.
fn full_input() -> InventorySnapshotInput {
    InventorySnapshotInput {
        membership_id: MembershipId::new("membership-1"),
        membership_type: MembershipType::new(u32::MAX),
        definition_version: DefinitionVersion::new("definitions-v1"),
        checked_at: Timestamp::new(200),
        source: SourceMarker::Profile,
        characters: vec![
            Character {
                id: "character-1".into(),
                name: Some("Nova".into()),
                class: Some("Warlock".into()),
                race: Some("Awoken".into()),
                gender: Some("Female".into()),
                light: Some(u16::MAX),
            },
            Character {
                id: "character-2".into(),
                name: None,
                class: None,
                race: None,
                gender: None,
                light: Some(0),
            },
        ],
        items: vec![
            ItemInstance {
                instance_id: "instance-1".into(),
                item_hash: ItemHash::new(-42),
                location: ItemLocation::Vault,
                quantity: u32::MAX,
                equipped: false,
                locked: true,
                power: Some(u16::MAX),
                perks: vec![
                    PerkSummary {
                        perk_hash: PerkHash::new(0),
                        active: true,
                        visible: true,
                    },
                    PerkSummary {
                        perk_hash: PerkHash::new(u32::MAX),
                        active: false,
                        visible: false,
                    },
                    // A duplicate hash is legal because a provider may summarize
                    // the same perk from more than one source.
                    PerkSummary {
                        perk_hash: PerkHash::new(0),
                        active: false,
                        visible: true,
                    },
                ],
            },
            ItemInstance {
                instance_id: "instance-2".into(),
                item_hash: ItemHash::new(7),
                location: ItemLocation::Character("character-1".into()),
                quantity: 0,
                equipped: true,
                locked: false,
                power: None,
                perks: vec![PerkSummary {
                    perk_hash: PerkHash::new(42),
                    active: false,
                    visible: true,
                }],
            },
            ItemInstance {
                instance_id: "instance-3".into(),
                item_hash: ItemHash::new(8),
                location: ItemLocation::Other("postmaster".into()),
                quantity: 1,
                equipped: false,
                locked: false,
                power: Some(0),
                perks: Vec::new(),
            },
        ],
    }
}

/// The canonical characters `full_input` round-trips to, ordered by identifier.
fn expected_characters() -> Vec<Character> {
    vec![
        Character {
            id: "character-1".into(),
            name: Some("Nova".into()),
            class: Some("Warlock".into()),
            race: Some("Awoken".into()),
            gender: Some("Female".into()),
            light: Some(u16::MAX),
        },
        Character {
            id: "character-2".into(),
            name: None,
            class: None,
            race: None,
            gender: None,
            light: Some(0),
        },
    ]
}

/// The canonical items `full_input` round-trips to, ordered by identifier.
fn expected_items() -> Vec<ItemInstance> {
    full_input().items
}

fn seeded_store() -> SqliteStore {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .replace_current(full_input())
        .expect("seed the committed projection");
    store
}

fn character_count(store: &SqliteStore) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM characters", [], |row| row.get(0))
        .expect("count characters")
}

fn item_count(store: &SqliteStore) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM items", [], |row| row.get(0))
        .expect("count items")
}

fn perk_count(store: &SqliteStore) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM item_perks", [], |row| row.get(0))
        .expect("count perks")
}

fn metadata_row_count(store: &SqliteStore) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM sync_metadata", [], |row| row.get(0))
        .expect("count metadata rows")
}

/// Enables the in-test constraint bypass so a real write cannot produce the
/// corrupt values the repository's own parse must reject.
fn bypass_constraints(store: &SqliteStore) {
    store
        .connection()
        .execute_batch("PRAGMA ignore_check_constraints = ON")
        .expect("enable the in-test constraint bypass");
}

fn manifest_replacement() -> DefinitionReplacementInput {
    DefinitionReplacementInput {
        definitions: ManifestDefinitions {
            version: DefinitionVersion::new("manifest-v1"),
            definitions: vec![ItemDefinition {
                hash: ItemHash::new(10),
                bucket_hash: None,
                name: "Alpha".into(),
                item_type: None,
                tier: None,
                element: None,
                slot: None,
                class_type: None,
                plug_category: None,
                tags: vec!["tag".into()],
                version: DefinitionVersion::new("manifest-v1"),
            }],
        },
        checked_at: Timestamp::new(100),
        source: SourceMarker::Manifest,
    }
}

#[test]
fn empty_store_reports_no_projection() {
    let store = SqliteStore::open_in_memory().expect("open in-memory store");
    assert_eq!(store.load_current().expect("read projection"), None);
}

#[test]
fn empty_collections_publish_as_a_complete_projection() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let mut empty = full_input();
    empty.characters.clear();
    empty.items.clear();

    let returned = store
        .replace_current(empty)
        .expect("an empty but structurally valid projection publishes");

    assert_eq!(returned.metadata.revision.value(), 1);
    assert!(returned.characters.is_empty());
    assert!(returned.items.is_empty());
    assert_eq!(character_count(&store), 0);
    assert_eq!(item_count(&store), 0);
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(returned),
        "an anchored projection with no children is still a committed projection"
    );
}

#[test]
fn first_publication_assigns_revision_one_and_round_trips_every_field() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let returned = store
        .replace_current(full_input())
        .expect("publish the first complete projection");

    assert_eq!(returned.metadata.revision.value(), 1);
    assert_eq!(returned.metadata.checked_at, Timestamp::new(200));
    assert_eq!(returned.metadata.source, SourceMarker::Profile);
    assert_eq!(returned.metadata.membership_id.as_str(), "membership-1");
    assert_eq!(returned.metadata.membership_type.value(), u32::MAX);
    assert_eq!(
        returned.metadata.definition_version.as_str(),
        "definitions-v1"
    );

    assert_eq!(returned.characters, expected_characters());
    assert_eq!(returned.items, expected_items());
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(returned),
        "the committed projection must equal the returned snapshot exactly"
    );
}

#[test]
fn character_and_item_permutations_retain_revision_and_canonical_order() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let first = store
        .replace_current(full_input())
        .expect("publish the first projection");
    assert_eq!(first.metadata.revision.value(), 1);

    let mut permuted = full_input();
    permuted.characters.reverse();
    permuted.items.reverse();
    let second = store
        .replace_current(permuted)
        .expect("republish in the opposite order");

    assert_eq!(second.metadata.revision.value(), 1);
    assert_eq!(second.characters, expected_characters());
    assert_eq!(second.items, expected_items());
    assert_eq!(store.load_current().expect("read projection"), Some(second));
}

#[test]
fn permuted_content_with_later_checked_at_retains_revision_and_refreshes_metadata() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .replace_current(full_input())
        .expect("publish the first projection");

    let mut permuted = full_input();
    permuted.characters.reverse();
    permuted.items.reverse();
    permuted.checked_at = Timestamp::new(9_000);
    let refreshed = store
        .replace_current(permuted)
        .expect("refresh with the same content");

    assert_eq!(refreshed.metadata.revision.value(), 1);
    assert_eq!(refreshed.metadata.checked_at, Timestamp::new(9_000));
    assert_eq!(refreshed.characters, expected_characters());
    assert_eq!(refreshed.items, expected_items());
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(refreshed)
    );
}

#[test]
fn every_normalized_field_and_location_advances_exactly_once() {
    type Mutation = (&'static str, fn(&mut InventorySnapshotInput));

    // Character index 1 is unreferenced, so renaming or removing it never makes
    // the candidate structurally invalid.
    let mutations: [Mutation; 24] = [
        ("membership_id", |value| {
            value.membership_id = MembershipId::new("membership-2");
        }),
        ("membership_type", |value| {
            value.membership_type = MembershipType::new(2);
        }),
        ("definition_version", |value| {
            value.definition_version = DefinitionVersion::new("definitions-v2");
        }),
        ("character.id", |value| {
            value.characters[1].id = "character-2-changed".into();
        }),
        ("character.name", |value| {
            value.characters[1].name = Some("name-changed".into());
        }),
        ("character.class", |value| {
            value.characters[1].class = Some("class-changed".into());
        }),
        ("character.race", |value| {
            value.characters[1].race = Some("race-changed".into());
        }),
        ("character.gender", |value| {
            value.characters[1].gender = Some("gender-changed".into());
        }),
        ("character.light", |value| {
            value.characters[1].light = Some(1);
        }),
        ("item.instance_id", |value| {
            value.items[0].instance_id = "instance-1-changed".into();
        }),
        ("item.item_hash", |value| {
            value.items[0].item_hash = ItemHash::new(9);
        }),
        ("item.location.vault-to-character", |value| {
            value.items[0].location = ItemLocation::Character("character-1".into());
        }),
        ("item.location.vault-to-other", |value| {
            value.items[0].location = ItemLocation::Other("postmaster".into());
        }),
        ("item.quantity", |value| {
            value.items[0].quantity = 5;
        }),
        ("item.equipped", |value| {
            value.items[0].equipped = true;
        }),
        ("item.locked", |value| {
            value.items[0].locked = false;
        }),
        ("item.power", |value| {
            value.items[0].power = Some(1);
        }),
        ("item.perks.order", |value| {
            value.items[0].perks.reverse();
        }),
        ("item.perks.perk_hash", |value| {
            value.items[0].perks[0].perk_hash = PerkHash::new(1);
        }),
        ("item.perks.active", |value| {
            value.items[0].perks[0].active = false;
        }),
        ("item.perks.visible", |value| {
            value.items[0].perks[0].visible = false;
        }),
        ("item.perks.from-empty", |value| {
            value.items[2].perks.push(PerkSummary {
                perk_hash: PerkHash::new(9),
                active: false,
                visible: true,
            });
        }),
        ("characters.membership", |value| {
            value.characters.pop();
        }),
        ("items.membership", |value| {
            value.items.pop();
        }),
    ];

    for (field, mutate) in mutations {
        let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
        store
            .replace_current(full_input())
            .expect("publish the base projection");
        let mut changed = full_input();
        mutate(&mut changed);

        assert_eq!(
            store
                .replace_current(changed)
                .expect("changed content publishes")
                .metadata
                .revision
                .value(),
            2,
            "changing {field} must advance the revision exactly once"
        );
    }
}

#[test]
fn invalid_source_duplicates_and_dangling_references_preserve_state() {
    let mut store = seeded_store();
    let committed = store.load_current().expect("read projection");

    let mut wrong_source = full_input();
    wrong_source.source = SourceMarker::Manifest;
    wrong_source.membership_id = MembershipId::new("membership-2");

    let mut duplicate_characters = full_input();
    duplicate_characters
        .characters
        .push(duplicate_characters.characters[0].clone());

    let mut duplicate_items = full_input();
    duplicate_items.items.push(duplicate_items.items[0].clone());

    let mut dangling = full_input();
    dangling.items[0].location = ItemLocation::Character("missing-character".into());

    let candidates = [
        ("invalid source", wrong_source),
        ("duplicate character ids", duplicate_characters),
        ("duplicate item ids", duplicate_items),
        ("dangling character location", dangling),
    ];

    for (label, candidate) in candidates {
        assert_eq!(
            store.replace_current(candidate),
            Err(AppError::new(ErrorCategory::ProviderValidation)),
            "{label} must be rejected as provider validation"
        );
        assert_eq!(
            store.load_current().expect("read projection"),
            committed,
            "{label} must preserve the committed projection"
        );
    }
}

#[test]
fn complete_replacement_removes_stale_rows_on_membership_and_set_change() {
    let mut store = seeded_store();
    assert_eq!(character_count(&store), 2);
    assert_eq!(item_count(&store), 3);

    let mut smaller = full_input();
    smaller.membership_id = MembershipId::new("membership-2");
    smaller
        .characters
        .retain(|character| character.id.as_str() == "character-1");
    smaller
        .items
        .retain(|item| item.instance_id.as_str() == "instance-1");

    let replaced = store
        .replace_current(smaller)
        .expect("publish the smaller projection");

    assert_eq!(replaced.metadata.revision.value(), 2);
    assert_eq!(replaced.metadata.membership_id.as_str(), "membership-2");
    assert_eq!(
        character_count(&store),
        1,
        "stale characters must be removed"
    );
    assert_eq!(item_count(&store), 1, "stale items must be removed");
    assert_eq!(
        perk_count(&store),
        3,
        "perks of removed items must be removed"
    );
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(replaced)
    );
}

#[test]
fn empty_perk_lists_round_trip_as_empty() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let mut input = full_input();
    for item in &mut input.items {
        item.perks.clear();
    }

    let returned = store
        .replace_current(input)
        .expect("a projection with no perks publishes");

    assert_eq!(returned.metadata.revision.value(), 1);
    assert!(returned.items.iter().all(|item| item.perks.is_empty()));
    assert_eq!(perk_count(&store), 0);
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(returned)
    );
}

#[test]
fn ordered_perks_round_trip_with_duplicates_and_both_hash_bounds() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let returned = store
        .replace_current(full_input())
        .expect("publish the full perk projection");

    assert_eq!(returned.items, expected_items());
    assert_eq!(perk_count(&store), 4);
    assert_eq!(returned.items[0].perks.len(), 3);
    assert_eq!(returned.items[0].perks[0].perk_hash, PerkHash::new(0));
    assert_eq!(
        returned.items[0].perks[1].perk_hash,
        PerkHash::new(u32::MAX)
    );
    assert_eq!(
        returned.items[0].perks[2].perk_hash,
        PerkHash::new(0),
        "a duplicated perk hash must be preserved, not deduplicated"
    );
    assert!(returned.items[2].perks.is_empty());
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(returned)
    );
}

#[test]
fn unchanged_exact_perks_retain_the_revision() {
    let mut store = seeded_store();
    let mut refreshed = full_input();
    refreshed.checked_at = Timestamp::new(9_000);

    let retained = store
        .replace_current(refreshed)
        .expect("refresh the identical perks");

    assert_eq!(retained.metadata.revision.value(), 1);
    assert_eq!(retained.metadata.checked_at, Timestamp::new(9_000));
    assert_eq!(retained.items, expected_items());
}

#[test]
fn stale_perks_are_removed_when_items_shrink_or_lose_their_perks() {
    let mut store = seeded_store();
    assert_eq!(perk_count(&store), 4);

    let mut shrunk = full_input();
    shrunk.items[0].perks.truncate(1);
    shrunk.items[1].perks.clear();
    let replaced = store
        .replace_current(shrunk)
        .expect("publish the shrunk perk lists");
    assert_eq!(replaced.metadata.revision.value(), 2);
    assert_eq!(perk_count(&store), 1);
    assert_eq!(replaced.items[0].perks.len(), 1);
    assert!(replaced.items[1].perks.is_empty());

    let mut cleared = full_input();
    for item in &mut cleared.items {
        item.perks.clear();
    }
    let replaced = store
        .replace_current(cleared)
        .expect("publish with every perk cleared");
    assert_eq!(replaced.metadata.revision.value(), 3);
    assert_eq!(perk_count(&store), 0);
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(replaced)
    );
}

#[test]
fn perk_write_failure_rolls_back_items_perks_and_metadata() {
    let mut store = seeded_store();
    let committed = store
        .load_current()
        .expect("read projection")
        .expect("projection present");
    assert_eq!(perk_count(&store), 4);

    // The replacement changes membership, writes the perk-bearing first item and
    // its three perks, and then fails on the second item's over-long unknown
    // location. The whole transaction must roll back to the prior projection.
    let mut failing = full_input();
    failing.membership_id = MembershipId::new("membership-2");
    failing.items[1].location = ItemLocation::Other("x".repeat(257));

    let error = store
        .replace_current(failing)
        .expect_err("a constraint failure must not commit");
    assert_eq!(error, storage_error());
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(committed),
        "the prior projection must survive the rollback"
    );
    assert_eq!(
        perk_count(&store),
        4,
        "rolled-back perk rows must not survive"
    );
    assert_eq!(item_count(&store), 3);
}

#[test]
fn corrupt_perk_rows_fail_closed() {
    // A non-contiguous ordinal list: deleting the zero ordinal leaves 1 and 2.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "DELETE FROM item_perks WHERE instance_id = 'instance-1' AND ordinal = 0",
            [],
        )
        .expect("drop the first perk ordinal");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A non-boolean active flag.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE item_perks SET active = 2 WHERE instance_id = 'instance-1' AND ordinal = 0",
            [],
        )
        .expect("write a non-boolean active flag");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A non-boolean visible flag.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE item_perks SET visible = -1 WHERE instance_id = 'instance-1' AND ordinal = 0",
            [],
        )
        .expect("write a negative visible flag");
    assert_eq!(store.load_current(), Err(storage_error()));

    // An out-of-range perk hash.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE item_perks SET perk_hash = 4294967296 \
             WHERE instance_id = 'instance-1' AND ordinal = 0",
            [],
        )
        .expect("write an out-of-range perk hash");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A negative ordinal.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE item_perks SET ordinal = -1 WHERE instance_id = 'instance-1' AND ordinal = 0",
            [],
        )
        .expect("write a negative ordinal");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A perk row that references an unknown item instance, foreign keys off.
    let store = seeded_store();
    store
        .connection()
        .execute_batch("PRAGMA foreign_keys = OFF")
        .expect("disable foreign keys for the corrupt write");
    store
        .connection()
        .execute(
            "INSERT INTO item_perks (instance_id, ordinal, perk_hash, active, visible) \
             VALUES ('ghost-item', 0, 1, 0, 0)",
            [],
        )
        .expect("write an orphan perk row");
    assert_eq!(store.load_current(), Err(storage_error()));
}

#[test]
fn overflow_retains_at_the_boundary_and_preserves_state() {
    let mut store = seeded_store();
    store
        .connection()
        .execute(
            "UPDATE sync_metadata SET revision = '18446744073709551615' \
             WHERE dataset = 'profile_inventory'",
            [],
        )
        .expect("seed the overflow boundary");

    // An unchanged refresh at the boundary retains the revision and refreshes
    // only the committed timestamp.
    let mut refreshed = full_input();
    refreshed.checked_at = Timestamp::new(9_000);
    let retained = store
        .replace_current(refreshed)
        .expect("an unchanged refresh must not overflow");
    assert_eq!(retained.metadata.revision.value(), u64::MAX);
    assert_eq!(retained.metadata.checked_at, Timestamp::new(9_000));

    // A genuine content change at the boundary overflows before any write.
    let mut changed = full_input();
    changed.membership_type = MembershipType::new(2);
    assert_eq!(store.replace_current(changed), Err(storage_error()));

    let loaded = store
        .load_current()
        .expect("read projection")
        .expect("projection present");
    assert_eq!(loaded.metadata.revision.value(), u64::MAX);
    assert_eq!(loaded.metadata.checked_at, Timestamp::new(9_000));
    assert_eq!(loaded.characters, retained.characters);
    assert_eq!(loaded.items, retained.items);
}

#[test]
fn constraint_failure_mid_replacement_rolls_back_profile_rows_and_metadata() {
    let mut store = seeded_store();
    let committed = store
        .load_current()
        .expect("read projection")
        .expect("projection present");

    // The membership and both collections change, so the replacement deletes
    // every prior row, upserts the anchor, writes the first character, and then
    // fails on a name that exceeds the schema bound.
    let mut failing = full_input();
    failing.membership_id = MembershipId::new("membership-2");
    failing.items.clear();
    failing.characters[1].name = Some("x".repeat(257));

    let error = store
        .replace_current(failing)
        .expect_err("a constraint failure must not commit");
    assert_eq!(error.category(), ErrorCategory::LocalStorage);
    assert_eq!(error, storage_error());

    assert_eq!(
        store.load_current().expect("read projection"),
        Some(committed),
        "the prior projection and metadata must survive the rollback"
    );
    assert_eq!(character_count(&store), 2);
    assert_eq!(item_count(&store), 3);
}

#[test]
fn partial_anchor_and_child_rows_fail_closed() {
    // Metadata without the active-profile anchor.
    let store = seeded_store();
    store
        .connection()
        .execute("DELETE FROM active_profile", [])
        .expect("remove the anchor");
    assert_eq!(store.load_current(), Err(storage_error()));

    // The active-profile anchor without metadata.
    let store = seeded_store();
    store
        .connection()
        .execute(
            "DELETE FROM sync_metadata WHERE dataset = 'profile_inventory'",
            [],
        )
        .expect("remove the metadata");
    assert_eq!(store.load_current(), Err(storage_error()));

    // Character rows without both anchor and metadata.
    let store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .connection()
        .execute(
            "INSERT INTO characters (character_id, name, class_name, race, gender, light) \
             VALUES ('orphan', 'n', 'c', 'r', 'g', 1)",
            [],
        )
        .expect("insert an orphan character");
    assert_eq!(store.load_current(), Err(storage_error()));

    // Item rows without both anchor and metadata.
    let store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .connection()
        .execute(
            "INSERT INTO items \
             (instance_id, item_hash, location_kind, location_character_id, location_other, \
              quantity, equipped, locked, power) \
             VALUES ('orphan-item', 1, 'vault', NULL, NULL, 1, 0, 0, NULL)",
            [],
        )
        .expect("insert an orphan item");
    assert_eq!(store.load_current(), Err(storage_error()));

    // Perk rows without any item, anchor, or metadata, foreign keys off.
    let store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .connection()
        .execute_batch("PRAGMA foreign_keys = OFF")
        .expect("disable foreign keys");
    store
        .connection()
        .execute(
            "INSERT INTO item_perks (instance_id, ordinal, perk_hash, active, visible) \
             VALUES ('ghost-perk', 0, 1, 0, 1)",
            [],
        )
        .expect("insert an orphan perk");
    assert_eq!(store.load_current(), Err(storage_error()));
}

#[test]
fn extra_active_profile_rows_fail_closed() {
    let store = seeded_store();
    bypass_constraints(&store);

    // The schema's primary key makes `singleton = 1` unique in a well-formed
    // store, but a corrupt database could carry an additional row with a
    // different discriminator. The singleton read must fail closed instead of
    // silently ignoring it.
    store
        .connection()
        .execute(
            "INSERT INTO active_profile (singleton, membership_id, membership_type) \
             VALUES (2, 'intruder', '1')",
            [],
        )
        .expect("insert an extra anchor row under the constraint bypass");
    assert_eq!(store.load_current(), Err(storage_error()));

    // Restore constraint enforcement: the corrupt two-row state still fails
    // closed, and the committed schema again rejects a fabricated anchor row.
    store
        .connection()
        .execute_batch("PRAGMA ignore_check_constraints = OFF")
        .expect("restore constraint enforcement");
    assert_eq!(store.load_current(), Err(storage_error()));
    assert!(
        store
            .connection()
            .execute(
                "INSERT INTO active_profile (singleton, membership_id, membership_type) \
                 VALUES (3, 'intruder-2', '1')",
                [],
            )
            .is_err(),
        "the committed schema must reject a fabricated anchor row"
    );
}

#[test]
fn malformed_decimal_source_and_range_values_fail_closed() {
    // A malformed revision.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE sync_metadata SET revision = 'not-a-number' \
             WHERE dataset = 'profile_inventory'",
            [],
        )
        .expect("write a malformed revision");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A non-canonical timestamp.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE sync_metadata SET checked_at = '01' \
             WHERE dataset = 'profile_inventory'",
            [],
        )
        .expect("write a non-canonical timestamp");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A mismatched provenance marker.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE sync_metadata SET source = 'manifest' \
             WHERE dataset = 'profile_inventory'",
            [],
        )
        .expect("write a mismatched source");
    assert_eq!(store.load_current(), Err(storage_error()));

    // An out-of-range membership type.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE active_profile SET membership_type = '4294967296'",
            [],
        )
        .expect("write an out-of-range membership type");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A non-canonical membership type.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute("UPDATE active_profile SET membership_type = '01'", [])
        .expect("write a non-canonical membership type");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A corrupt singleton discriminator.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute("UPDATE active_profile SET singleton = 0", [])
        .expect("write a corrupt singleton");
    assert_eq!(store.load_current(), Err(storage_error()));
}

#[test]
fn invalid_boolean_location_and_reference_values_fail_closed() {
    // An out-of-range light value.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE characters SET light = 70000 WHERE character_id = 'character-2'",
            [],
        )
        .expect("write an out-of-range light");
    assert_eq!(store.load_current(), Err(storage_error()));

    // An out-of-range quantity.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE items SET quantity = 4294967296 WHERE instance_id = 'instance-1'",
            [],
        )
        .expect("write an out-of-range quantity");
    assert_eq!(store.load_current(), Err(storage_error()));

    // An out-of-range power value.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE items SET power = 70000 WHERE instance_id = 'instance-1'",
            [],
        )
        .expect("write an out-of-range power");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A boolean that is not exactly 0 or 1.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE items SET equipped = 2 WHERE instance_id = 'instance-1'",
            [],
        )
        .expect("write a non-boolean flag");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A vault location that also carries a character reference.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE items SET location_character_id = 'character-1' \
             WHERE instance_id = 'instance-1'",
            [],
        )
        .expect("write a mixed location");
    assert_eq!(store.load_current(), Err(storage_error()));

    // An unknown location kind.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE items SET location_kind = 'future' WHERE instance_id = 'instance-1'",
            [],
        )
        .expect("write an unknown location kind");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A character reference with no matching character, with foreign keys off.
    let store = seeded_store();
    store
        .connection()
        .execute_batch("PRAGMA foreign_keys = OFF")
        .expect("disable foreign keys for the corrupt write");
    store
        .connection()
        .execute(
            "UPDATE items \
             SET location_kind = 'character', location_character_id = 'ghost', \
                 location_other = NULL \
             WHERE instance_id = 'instance-1'",
            [],
        )
        .expect("write a dangling reference");
    assert_eq!(store.load_current(), Err(storage_error()));
}

#[test]
fn file_close_and_reopen_preserves_exact_projection_and_revision() {
    let temp = TempDatabase::new();

    let committed = {
        let mut store = SqliteStore::open(temp.path()).expect("open file store");
        store
            .replace_current(full_input())
            .expect("publish to the file store")
    };

    let mut store = SqliteStore::open(temp.path()).expect("reopen file store");
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(committed.clone())
    );

    // The reconstructed committed content equals the original input, so an
    // unchanged refresh after reopen retains the revision.
    let mut refreshed = full_input();
    refreshed.checked_at = Timestamp::new(9_000);
    let retained = store
        .replace_current(refreshed)
        .expect("refresh after reopen");
    assert_eq!(retained.metadata.revision.value(), 1);
    assert_eq!(retained.metadata.checked_at, Timestamp::new(9_000));
    assert_eq!(retained.characters, committed.characters);
    assert_eq!(retained.items, committed.items);
}

#[test]
fn manifest_definitions_and_metadata_are_untouched_by_profile_writes() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let manifest = store
        .replace_definitions(manifest_replacement())
        .expect("publish Manifest definitions");
    store
        .replace_current(full_input())
        .expect("publish the profile projection");

    let mut changed = full_input();
    changed.membership_id = MembershipId::new("membership-2");
    store
        .replace_current(changed)
        .expect("replace the profile projection");

    assert_eq!(metadata_row_count(&store), 2);
    assert_eq!(
        store.current_metadata().expect("read Manifest metadata"),
        Some(manifest),
        "the Manifest metadata row must not change"
    );
    assert_eq!(
        store
            .current_definition_version()
            .expect("read Manifest version")
            .as_str(),
        "manifest-v1"
    );
    assert_eq!(
        store
            .lookup_definition(ItemHash::new(10))
            .expect("lookup definition"),
        Resolution::Unique(manifest_replacement().definitions.definitions[0].clone()),
        "the Manifest catalog must not change"
    );

    // The profile projection itself remains readable and canonical.
    let loaded = store
        .load_current()
        .expect("read projection")
        .expect("projection present");
    assert_eq!(loaded.metadata.revision.value(), 2);
    assert_eq!(loaded.metadata.membership_id.as_str(), "membership-2");
}

#[test]
fn errors_and_debug_never_leak_stored_values() {
    let mut store = seeded_store();
    let committed = store.load_current().expect("read projection");

    let sentinel = "provider-secret-sentinel";
    let mut bad = full_input();
    bad.characters[1].name = Some(sentinel.repeat(20));
    let error = store
        .replace_current(bad)
        .expect_err("an over-long name is rejected");

    assert_eq!(error, storage_error());
    for leaked in [
        sentinel,
        "sqlite",
        "constraint",
        "CHECK",
        "characters",
        "items",
        "active_profile",
        "sync_metadata",
    ] {
        assert!(
            !error.to_string().contains(leaked),
            "error leaked {leaked}: {error}"
        );
    }
    assert!(!format!("{error:?}").contains(sentinel));
    assert_eq!(
        store.load_current().expect("read projection"),
        committed,
        "a rejected replacement must preserve the committed projection"
    );
}
