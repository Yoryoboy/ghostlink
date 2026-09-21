//! Focused contract tests for the transactional SQLite definition repository.
//!
//! Every test is offline, credential-free, and deterministic. File-backed tests
//! use a private temporary path derived from the process id and an atomic
//! counter, and always remove the database and its sidecars.

use super::{LATEST_SCHEMA_VERSION, SqliteStore};
use destiny_core::{
    AppError, BucketHash, Dataset, DefinitionReplacementInput, DefinitionRepository,
    DefinitionVersion, ErrorCategory, ItemDefinition, ItemHash, ManifestDefinitions, Resolution,
    SourceMarker, Timestamp,
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
            "destiny-definitions-{}-{sequence}.db",
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

fn item(hash: i64, name: &str) -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(hash),
        bucket_hash: None,
        name: name.into(),
        item_type: None,
        tier: None,
        element: None,
        tags: Vec::new(),
        version: DefinitionVersion::new("v1"),
    }
}

fn versioned(mut definition: ItemDefinition, version: &str) -> ItemDefinition {
    definition.version = DefinitionVersion::new(version);
    definition
}

fn full_definition(hash: i64, tags: &[&str]) -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(hash),
        bucket_hash: Some(BucketHash::new(1_498_876_634)),
        name: "Suros Regime".into(),
        item_type: Some("Auto Rifle".into()),
        tier: Some("Exotic".into()),
        element: Some("Solar".into()),
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        version: DefinitionVersion::new("manifest-2026-09-21"),
    }
}

fn replacement(version: &str, definitions: Vec<ItemDefinition>) -> DefinitionReplacementInput {
    DefinitionReplacementInput {
        definitions: ManifestDefinitions {
            version: DefinitionVersion::new(version),
            definitions,
        },
        checked_at: Timestamp::new(200),
        source: SourceMarker::Manifest,
    }
}

fn base_manifest() -> ManifestDefinitions {
    ManifestDefinitions {
        version: DefinitionVersion::new("v1"),
        definitions: vec![
            ItemDefinition {
                hash: ItemHash::new(10),
                bucket_hash: Some(BucketHash::new(100)),
                name: "Alpha".into(),
                item_type: Some("Auto Rifle".into()),
                tier: Some("Legendary".into()),
                element: Some("Void".into()),
                tags: vec!["first".into(), "second".into()],
                version: DefinitionVersion::new("v1"),
            },
            ItemDefinition {
                hash: ItemHash::new(20),
                bucket_hash: None,
                name: "Beta".into(),
                item_type: None,
                tier: None,
                element: None,
                tags: Vec::new(),
                version: DefinitionVersion::new("v1"),
            },
        ],
    }
}

fn definition_count(store: &SqliteStore) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM definitions", [], |row| row.get(0))
        .expect("count definitions")
}

fn tag_count(store: &SqliteStore) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM definition_tags", [], |row| row.get(0))
        .expect("count definition tags")
}

#[test]
fn empty_store_reads_match_fake_semantics() {
    let store = SqliteStore::open_in_memory().expect("open in-memory store");

    assert_eq!(store.current_metadata().expect("read metadata"), None);
    assert_eq!(
        store.current_definition_version(),
        Err(storage_error()),
        "an empty store reports local storage, matching the fake"
    );
    assert_eq!(
        store.lookup_definition(ItemHash::new(7)).expect("lookup"),
        Resolution::Missing
    );
}

#[test]
fn first_publication_assigns_revision_one_and_round_trips_exactly() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let alpha = full_definition(-42, &["weapon", "exotic", "search term"]);
    let beta = versioned(item(2_522_695, "Beta"), "manifest-2026-09-21");

    let metadata = store
        .replace_definitions(replacement(
            "manifest-2026-09-21",
            vec![alpha.clone(), beta.clone()],
        ))
        .expect("publish the first complete set");

    assert_eq!(metadata.dataset, Dataset::ManifestDefinitions);
    assert_eq!(metadata.source, SourceMarker::Manifest);
    assert_eq!(metadata.revision.value(), 1);
    assert_eq!(metadata.checked_at, Timestamp::new(200));
    assert_eq!(metadata.definition_version.as_str(), "manifest-2026-09-21");

    assert_eq!(
        store.current_metadata().expect("read metadata"),
        Some(metadata)
    );
    assert_eq!(
        store
            .current_definition_version()
            .expect("read version")
            .as_str(),
        "manifest-2026-09-21"
    );
    assert_eq!(
        store.lookup_definition(alpha.hash).expect("lookup alpha"),
        Resolution::Unique(alpha.clone()),
        "every normalized field and ordered tag must round-trip exactly"
    );
    assert_eq!(
        store.lookup_definition(beta.hash).expect("lookup beta"),
        Resolution::Unique(beta.clone())
    );
    assert_eq!(
        store
            .lookup_definition(ItemHash::new(9_999))
            .expect("lookup"),
        Resolution::Missing
    );
}

#[test]
fn identical_replacement_retains_revision() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let input = replacement("v1", vec![item(10, "Alpha"), item(20, "Beta")]);

    let first = store
        .replace_definitions(input.clone())
        .expect("publish the first set");
    let second = store
        .replace_definitions(input)
        .expect("republish the same set");

    assert_eq!(first, second);
    assert_eq!(second.revision.value(), 1);
    assert_eq!(definition_count(&store), 2);
}

#[test]
fn definition_order_is_not_content_but_tag_order_is() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let mut alpha = item(10, "Alpha");
    alpha.tags = vec!["a".into(), "b".into()];
    let beta = item(20, "Beta");

    let first = store
        .replace_definitions(replacement("v1", vec![alpha.clone(), beta.clone()]))
        .expect("publish the first set");
    assert_eq!(first.revision.value(), 1);

    // The schema stores no definition ordinal and reads are ordered by primary
    // key, so identity is the normalized set, not the input order.
    let reordered = store
        .replace_definitions(replacement("v1", vec![beta.clone(), alpha.clone()]))
        .expect("republish the same set in a different order");
    assert_eq!(reordered.revision.value(), 1);

    // Tags carry an ordinal and round-trip in order, so reordering them is a
    // normalized content change.
    let mut retagged = alpha.clone();
    retagged.tags = vec!["b".into(), "a".into()];
    let changed = store
        .replace_definitions(replacement("v1", vec![retagged, beta]))
        .expect("republish with reordered tags");
    assert_eq!(changed.revision.value(), 2);

    match store
        .lookup_definition(ItemHash::new(10))
        .expect("lookup alpha")
    {
        Resolution::Unique(definition) => {
            assert_eq!(definition.tags, vec!["b".to_owned(), "a".to_owned()]);
        }
        other => panic!("expected a unique definition, got {other:?}"),
    }
}

#[test]
fn later_checked_at_retains_revision_and_refreshes_metadata() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let base = replacement("v1", vec![item(10, "Alpha")]);

    let first = store.replace_definitions(base.clone()).expect("publish");
    assert_eq!(first.revision.value(), 1);
    assert_eq!(first.checked_at, Timestamp::new(200));

    let mut refreshed = base;
    refreshed.checked_at = Timestamp::new(9_000);
    let second = store.replace_definitions(refreshed).expect("refresh");

    assert_eq!(
        second.revision.value(),
        1,
        "freshness alone must not advance"
    );
    assert_eq!(second.checked_at, Timestamp::new(9_000));
    assert_eq!(second.source, SourceMarker::Manifest);
    assert_eq!(
        store.current_metadata().expect("read metadata"),
        Some(second)
    );
}

#[test]
fn changing_each_normalized_field_advances_exactly_once() {
    type Mutation = (&'static str, fn(&mut ManifestDefinitions));

    let mutations: [Mutation; 11] = [
        ("bucket_hash", |set| {
            set.definitions[0].bucket_hash = Some(BucketHash::new(101));
        }),
        ("name", |set| {
            set.definitions[0].name = "Alpha Changed".into();
        }),
        ("item_type", |set| {
            set.definitions[0].item_type = None;
        }),
        ("tier", |set| {
            set.definitions[0].tier = None;
        }),
        ("element", |set| {
            set.definitions[0].element = None;
        }),
        ("tag", |set| {
            set.definitions[0].tags[0] = "renamed".into();
        }),
        ("tag order", |set| {
            set.definitions[0].tags.swap(0, 1);
        }),
        ("definition hash", |set| {
            set.definitions[0].hash = ItemHash::new(11);
        }),
        ("added definition", |set| {
            set.definitions.push(item(30, "Gamma"));
        }),
        ("removed definition", |set| {
            set.definitions.pop();
        }),
        ("definition version", |set| {
            set.version = DefinitionVersion::new("v2");
            for definition in &mut set.definitions {
                definition.version = DefinitionVersion::new("v2");
            }
        }),
    ];

    for (field, mutate) in mutations {
        let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
        store
            .replace_definitions(replacement("v1", base_manifest().definitions))
            .expect("publish the base set");

        let mut changed = base_manifest();
        mutate(&mut changed);
        let metadata = store
            .replace_definitions(DefinitionReplacementInput {
                definitions: changed,
                checked_at: Timestamp::new(200),
                source: SourceMarker::Manifest,
            })
            .expect("changed content publishes");

        assert_eq!(
            metadata.revision.value(),
            2,
            "changing {field} must advance the revision exactly once"
        );
        assert_eq!(metadata.checked_at, Timestamp::new(200));
    }
}

#[test]
fn invalid_replacements_are_provider_validation_and_preserve_state() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let committed = store
        .replace_definitions(replacement("v1", vec![item(10, "Alpha")]))
        .expect("seed a committed set");
    assert_eq!(committed.revision.value(), 1);

    let mut wrong_source = replacement("v2", vec![versioned(item(99, "Gamma"), "v2")]);
    wrong_source.source = SourceMarker::Profile;
    let empty = replacement("v2", Vec::new());
    let duplicate = replacement(
        "v2",
        vec![
            versioned(item(20, "Delta"), "v2"),
            versioned(item(20, "Epsilon"), "v2"),
        ],
    );
    let mixed = replacement("v2", vec![item(30, "Zeta")]);

    let candidates = [
        ("invalid source", wrong_source),
        ("empty set", empty),
        ("duplicate hashes", duplicate),
        ("mixed version", mixed),
    ];

    for (label, candidate) in candidates {
        assert_eq!(
            store.replace_definitions(candidate),
            Err(AppError::new(ErrorCategory::ProviderValidation)),
            "{label} must be rejected as provider validation"
        );
        assert_eq!(
            store.current_metadata().expect("read metadata"),
            Some(committed.clone()),
            "{label} must not change metadata"
        );
        assert_eq!(
            store
                .current_definition_version()
                .expect("read version")
                .as_str(),
            "v1",
            "{label} must not change the version"
        );
        assert_eq!(
            store.lookup_definition(ItemHash::new(10)).expect("lookup"),
            Resolution::Unique(item(10, "Alpha")),
            "{label} must preserve the committed catalog"
        );
        assert_eq!(
            store.lookup_definition(ItemHash::new(20)).expect("lookup"),
            Resolution::Missing,
            "{label} must not add rows"
        );
        assert_eq!(
            store.lookup_definition(ItemHash::new(99)).expect("lookup"),
            Resolution::Missing
        );
    }
}

#[test]
fn overflow_retains_at_the_boundary_and_preserves_state() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let original = replacement("v1", vec![item(10, "Alpha")]);
    store
        .replace_definitions(original.clone())
        .expect("publish the base set");
    store
        .connection_mut()
        .execute(
            "UPDATE sync_metadata SET revision = '18446744073709551615' \
             WHERE dataset = 'manifest_definitions'",
            [],
        )
        .expect("seed the overflow boundary");

    // An unchanged refresh at the boundary retains the revision and refreshes
    // only the committed timestamp.
    let mut refreshed = original.clone();
    refreshed.checked_at = Timestamp::new(9_000);
    let retained = store.replace_definitions(refreshed).expect("refresh");
    assert_eq!(retained.revision.value(), u64::MAX);
    assert_eq!(retained.checked_at, Timestamp::new(9_000));

    // A genuine content change at the boundary overflows before any mutation.
    let changed = replacement("v2", vec![versioned(item(10, "Alpha"), "v2")]);
    assert_eq!(store.replace_definitions(changed), Err(storage_error()));

    let metadata = store
        .current_metadata()
        .expect("read metadata")
        .expect("metadata present");
    assert_eq!(metadata.revision.value(), u64::MAX);
    assert_eq!(metadata.checked_at, Timestamp::new(9_000));
    assert_eq!(
        store
            .current_definition_version()
            .expect("read version")
            .as_str(),
        "v1"
    );
    assert_eq!(
        store.lookup_definition(ItemHash::new(10)).expect("lookup"),
        Resolution::Unique(item(10, "Alpha"))
    );
}

#[test]
fn constraint_failure_mid_replacement_rolls_back_catalog_and_metadata() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let mut alpha = item(10, "Alpha");
    alpha.tags = vec!["kept".into()];
    let committed = store
        .replace_definitions(replacement("v1", vec![alpha, item(20, "Beta")]))
        .expect("seed a committed set");

    // The first definition writes successfully, then a tag that exceeds the
    // schema bound fails the transaction partway through the replacement.
    let overflow_tag = "x".repeat(65);
    let mut alpha_changed = item(10, "Alpha Changed");
    alpha_changed.tags = vec!["kept".into(), overflow_tag];
    let mut beta_changed = item(20, "Beta Changed");
    beta_changed.tags = vec!["ok".into()];
    let failing = replacement(
        "v2",
        vec![
            versioned(alpha_changed, "v2"),
            versioned(beta_changed, "v2"),
        ],
    );

    let error = store
        .replace_definitions(failing)
        .expect_err("a constraint failure must not commit");
    assert_eq!(error.category(), ErrorCategory::LocalStorage);
    assert_eq!(error.to_string(), storage_error().to_string());

    assert_eq!(
        store.current_metadata().expect("read metadata"),
        Some(committed)
    );
    assert_eq!(
        store
            .current_definition_version()
            .expect("read version")
            .as_str(),
        "v1"
    );
    let mut expected_alpha = item(10, "Alpha");
    expected_alpha.tags = vec!["kept".into()];
    assert_eq!(
        store.lookup_definition(ItemHash::new(10)).expect("lookup"),
        Resolution::Unique(expected_alpha)
    );
    assert_eq!(
        store.lookup_definition(ItemHash::new(20)).expect("lookup"),
        Resolution::Unique(item(20, "Beta"))
    );
    assert_eq!(definition_count(&store), 2);
    assert_eq!(
        tag_count(&store),
        1,
        "the replacement's tag rows must not survive the rollback"
    );
}

#[test]
fn complete_replacement_removes_stale_definitions_and_tags() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let mut alpha = item(10, "Alpha");
    alpha.tags = vec!["a".into(), "b".into()];
    let mut beta = item(20, "Beta");
    beta.tags = vec!["c".into()];
    store
        .replace_definitions(replacement("v1", vec![alpha, beta]))
        .expect("publish the first set");
    assert_eq!(tag_count(&store), 3);

    let gamma = versioned(item(30, "Gamma"), "v2");
    let metadata = store
        .replace_definitions(replacement("v2", vec![gamma.clone()]))
        .expect("publish the replacement");
    assert_eq!(metadata.revision.value(), 2);

    assert_eq!(
        store.lookup_definition(ItemHash::new(10)).expect("lookup"),
        Resolution::Missing
    );
    assert_eq!(
        store.lookup_definition(ItemHash::new(20)).expect("lookup"),
        Resolution::Missing
    );
    assert_eq!(
        store.lookup_definition(ItemHash::new(30)).expect("lookup"),
        Resolution::Unique(gamma)
    );
    assert_eq!(definition_count(&store), 1);
    assert_eq!(tag_count(&store), 0, "stale tags must be removed");
    assert_eq!(
        store
            .current_definition_version()
            .expect("read version")
            .as_str(),
        "v2"
    );
}

#[test]
fn file_close_and_reopen_preserves_exact_reads_and_revision() {
    let temp = TempDatabase::new();
    let alpha = full_definition(-42, &["weapon", "exotic"]);
    let input = replacement("manifest-2026-09-21", vec![alpha.clone()]);

    let committed = {
        let mut store = SqliteStore::open(temp.path()).expect("open file store");
        store
            .replace_definitions(input.clone())
            .expect("publish to the file store")
    };

    let mut store = SqliteStore::open(temp.path()).expect("reopen file store");
    assert_eq!(
        store.schema_version().expect("read schema version"),
        LATEST_SCHEMA_VERSION
    );
    assert_eq!(
        store.current_metadata().expect("read metadata"),
        Some(committed)
    );
    assert_eq!(
        store
            .current_definition_version()
            .expect("read version")
            .as_str(),
        "manifest-2026-09-21"
    );
    assert_eq!(
        store.lookup_definition(alpha.hash).expect("lookup"),
        Resolution::Unique(alpha.clone())
    );

    // The reconstructed committed content equals the original input, so an
    // unchanged refresh after reopen retains the revision.
    let mut refreshed = input;
    refreshed.checked_at = Timestamp::new(9_000);
    let retained = store.replace_definitions(refreshed).expect("refresh");
    assert_eq!(retained.revision.value(), 1);
    assert_eq!(retained.checked_at, Timestamp::new(9_000));
    assert_eq!(
        store.lookup_definition(alpha.hash).expect("lookup"),
        Resolution::Unique(alpha)
    );
}

#[test]
fn reads_and_failures_are_deterministic_and_sanitized() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let mut alpha = item(10, "Alph\u{e1}");
    alpha.tags = vec!["First".into(), "second".into()];
    let mut beta = item(20, "Beta");
    beta.item_type = Some("Auto Rifle".into());
    store
        .replace_definitions(replacement("v1", vec![beta.clone(), alpha.clone()]))
        .expect("publish the set");

    for _ in 0..3 {
        assert_eq!(
            store
                .current_definition_version()
                .expect("read version")
                .as_str(),
            "v1"
        );
        assert_eq!(
            store.lookup_definition(ItemHash::new(10)).expect("lookup"),
            Resolution::Unique(alpha.clone()),
            "tag order and exact casing must be stable across reads"
        );
        assert_eq!(
            store.lookup_definition(ItemHash::new(20)).expect("lookup"),
            Resolution::Unique(beta.clone())
        );
        assert_eq!(
            store.lookup_definition(ItemHash::new(30)).expect("lookup"),
            Resolution::Missing
        );
    }

    // A sentinel placed in an over-long tag must never reach the sanitized error.
    let sentinel = "provider-secret-sentinel";
    let mut bad = alpha.clone();
    bad.tags = vec![format!("{sentinel}{sentinel}{sentinel}{sentinel}")];
    let error = store
        .replace_definitions(replacement("v1", vec![bad]))
        .expect_err("an over-long tag is rejected");
    assert_eq!(error, storage_error());
    for leaked in [
        sentinel,
        "sqlite",
        "constraint",
        "definitions",
        "definition_tags",
        "CHECK",
        "tag",
    ] {
        assert!(
            !error.to_string().contains(leaked),
            "error leaked {leaked}: {error}"
        );
    }
    assert!(!format!("{error:?}").contains(sentinel));
    assert_eq!(
        store.lookup_definition(ItemHash::new(10)).expect("lookup"),
        Resolution::Unique(alpha)
    );

    // No stored column is a generic payload column.
    for table in ["definitions", "definition_tags", "sync_metadata"] {
        let mut statement = store
            .connection()
            .prepare(&format!(
                "SELECT name, type FROM pragma_table_info('{table}')"
            ))
            .expect("prepare column query");
        let columns: Vec<(String, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("run column query")
            .map(|row| row.expect("read column row"))
            .collect();
        assert!(!columns.is_empty());
        for (column, column_type) in columns {
            assert!(
                column_type == "TEXT" || column_type == "INTEGER",
                "{table}.{column} must not be a raw payload column"
            );
        }
    }
}

#[test]
fn corrupted_metadata_fails_closed_without_weakening_the_schema() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .replace_definitions(replacement("v1", vec![item(10, "Alpha")]))
        .expect("seed a committed set");

    // `ignore_check_constraints` simulates on-disk corruption for this test
    // only. The committed schema still carries its canonical-decimal CHECK
    // constraints, so a real write cannot produce these values; the
    // repository's own parse must therefore fail closed on them.
    store
        .connection()
        .execute_batch("PRAGMA ignore_check_constraints = ON")
        .expect("enable the in-test constraint bypass");

    store
        .connection()
        .execute(
            "UPDATE sync_metadata SET revision = 'not-a-number' \
             WHERE dataset = 'manifest_definitions'",
            [],
        )
        .expect("write a malformed revision");
    assert_eq!(
        store
            .current_metadata()
            .expect_err("a malformed revision must fail closed"),
        storage_error()
    );

    store
        .connection()
        .execute(
            "UPDATE sync_metadata SET revision = '1', checked_at = '01' \
             WHERE dataset = 'manifest_definitions'",
            [],
        )
        .expect("write a non-canonical timestamp");
    assert_eq!(
        store
            .current_definition_version()
            .expect_err("a non-canonical timestamp must fail closed"),
        storage_error()
    );
}
