//! Synthetic offline CLI journey through QueryOperations, QueryService and SQLite.
use destiny_adapters::sqlite::SqliteStore;
use destiny_app::test_support::QueuedProfileGateway;
use destiny_app::*;
use destiny_cli::OutputMode;
use destiny_cli::postmaster_output::{postmaster_json, render_postmaster};
use destiny_cli::{ReadFreshness, run};

const NOW: Timestamp = Timestamp::new(100);
const POLICY: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };

fn snapshot() -> InventorySnapshotInput {
    InventorySnapshotInput {
        profile_currencies_coverage: ProfileCurrenciesCoverage::Available,
        stacks: vec![],
        membership_id: MembershipId::new("789"),
        membership_type: MembershipType::new(3),
        checked_at: NOW,
        source: SourceMarker::Profile,
        definition_version: DefinitionVersion::new("v1"),
        characters: ["456", "457"]
            .into_iter()
            .map(|id| Character {
                id: CharacterId::new(id),
                name: None,
                class: Some(if id == "456" { "Hunter" } else { "Titan" }.into()),
                race: None,
                gender: None,
                light: None,
                date_last_played: None,
            })
            .collect(),
        items: vec![ItemInstance {
            instance_id: ItemInstanceId::new("123"),
            item_hash: ItemHash::new(1),
            location: ItemLocation::Postmaster(CharacterId::new("456")),
            quantity: 2,
            equipped: false,
            locked: false,
            power: None,
            stats: vec![],
            plugs: vec![],
            perks: vec![],
            energy: None,
            gear_tier: None,
            sockets: Vec::new(),
            transfer_status: None,
            available_plugs: vec![],
        }],
        plug_set_entries: vec![],
    }
}

fn service() -> QueryService<QueuedProfileGateway, SqliteStore> {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions: vec![ItemDefinition {
                    non_transferrable: None,
                    does_postmaster_pull_have_side_effects: None,
                    is_instance_item: None,
                    max_stack_size: None,
                    stack_unique_label: None,
                    hash: ItemHash::new(2),
                    bucket_hash: Some(BucketHash::new(999)),
                    name: "Unrelated definition".into(),
                    item_type: None,
                    tier: None,
                    element: None,
                    slot: None,
                    class_type: None,
                    plug_category: None,
                    tags: vec![],
                    version: DefinitionVersion::new("v1"),
                }],
            },
            checked_at: NOW,
            source: SourceMarker::Manifest,
        })
        .unwrap();
    store.replace_current(snapshot()).unwrap();
    QueryService::new(ProfileSyncService::new(QueuedProfileGateway::new(), store))
}

#[test]
fn postmaster_offline_pipeline_trait_and_cli_success_and_refresh_refusal() {
    let mut service = service();
    let operations: &mut dyn QueryOperations = &mut service;
    let result = operations.postmaster(None, POLICY, NOW).unwrap();
    assert_eq!(result.characters.len(), 2);
    for args in [
        vec!["ghostlink", "postmaster", "--json"],
        vec!["ghostlink", "postmaster", "--character", "HuNtEr", "--json"],
    ] {
        let output = run(
            &mut service,
            args,
            ReadFreshness {
                policy: POLICY,
                now: NOW,
            },
        );
        assert_eq!(output.exit_code, 0);
        assert!(output.stderr.is_empty());
        let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(value["characters"][0]["instances"][0]["instance_id"], "123");
    }
    let refusal = run(
        &mut service,
        ["ghostlink", "postmaster", "--json"],
        ReadFreshness {
            policy: POLICY,
            now: Timestamp::new(1000),
        },
    );
    assert_ne!(refusal.exit_code, 0);
    assert!(refusal.stdout.is_empty());
    assert!(serde_json::from_str::<serde_json::Value>(&refusal.stderr).is_ok());
}

#[test]
fn postmaster_exact_json_and_text_preserve_observations() {
    let mut result = service().postmaster(None, POLICY, NOW).unwrap();
    result.characters[0].class = Some("Hun\nter\u{1b}".into());
    result.characters[0].instances[0].name = Some("Name\n\"quoted\"".into());
    let stack = PostmasterStack {
        item_hash: ItemHash::new(99),
        name: None,
        bucket_hash: BucketHash::new(215593132),
        quantity: 7,
    };
    result.characters[0].stacks = vec![stack.clone(), stack];
    assert_eq!(
        postmaster_json(&result),
        serde_json::json!({
            "kind": "postmaster",
            "metadata": {"checked_at": 100, "freshness": "fresh", "revision": 1, "source": "profile"},
            "profile_currencies_coverage": "available",
            "characters": [
                {"character_id": "456", "class": "Hun\nter\u{1b}", "instances": [{"instance_id": "123", "item_hash": 1, "name": "Name\n\"quoted\"", "quantity": 2}], "stacks": [
                    {"item_hash": 99, "name": null, "bucket_hash": 215593132, "quantity": 7},
                    {"item_hash": 99, "name": null, "bucket_hash": 215593132, "quantity": 7}]},
                {"character_id": "457", "class": "Titan", "instances": [], "stacks": []}
            ]
        })
    );
    let text = render_postmaster(OutputMode::Human, &result);
    assert!(!text.contains('\u{1b}'));
    assert!(text.contains("Hun?ter?"));
    assert!(text.contains("instance_id: 123"));
    assert_eq!(text.matches("bucket_hash: 215593132").count(), 2);
    assert!(text.contains("Titan (457)"));
    assert!(text.contains("Instances: none\n  Stacks: none"));
    assert_eq!(
        text,
        "Postmaster committed observations (not a live inventory guarantee)\nHun?ter? (456) — Postmaster\n  Instances:\n    Name?\"quoted\" | item_hash: 1 | quantity: 2 | instance_id: 123\n  Stacks:\n    hash 99 | item_hash: 99 | quantity: 7 | bucket_hash: 215593132\n    hash 99 | item_hash: 99 | quantity: 7 | bucket_hash: 215593132\nTitan (457) — Postmaster\n  Instances: none\n  Stacks: none\nStacks are anonymous observations, not stable pull selectors. No pull feature is exposed.\nprofile_currencies_coverage: available (component 103 only, not Postmaster completeness)\nrevision: 1\nchecked_at: 100\nfreshness: fresh"
    );
    for (coverage, label) in [
        (ProfileCurrenciesCoverage::NotObserved, "not_observed"),
        (ProfileCurrenciesCoverage::Unavailable, "unavailable"),
        (ProfileCurrenciesCoverage::Available, "available"),
    ] {
        result.profile_currencies_coverage = coverage;
        assert_eq!(
            postmaster_json(&result)["profile_currencies_coverage"],
            label
        );
        assert!(render_postmaster(OutputMode::Human, &result).contains(label));
    }
    result.characters.clear();
    assert!(render_postmaster(OutputMode::Human, &result).contains("No owned characters"));
}
