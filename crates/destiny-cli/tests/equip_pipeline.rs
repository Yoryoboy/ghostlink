//! Offline CLI adapter journey over the real planner/executor and SQLite.
use destiny_adapters::sqlite::SqliteStore;
use destiny_app::test_support::{
    DeterministicPlanIds, FakeClock, QueuedEquipGateway, QueuedProfileGateway,
    QueuedTransferGateway,
};
use destiny_app::*;
use destiny_cli::command::EquipArgs;
use destiny_cli::equip::equip_item;
use destiny_cli::equip_output::{EquipStatus, ProfileRefresh};

const NOW: Timestamp = Timestamp::new(100);
const POLICY: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };

fn character(id: &str) -> Character {
    Character {
        id: CharacterId::new(id),
        name: None,
        class: Some("Hunter".into()),
        race: None,
        gender: None,
        light: None,
        date_last_played: None,
    }
}

fn target() -> ItemLocation {
    ItemLocation::Character(CharacterId::new("456"))
}

fn membership() -> DestinyMembership {
    DestinyMembership::new(MembershipType::new(3), MembershipId::new("789")).unwrap()
}

fn snapshot(location: ItemLocation, equipped: bool) -> InventorySnapshotInput {
    InventorySnapshotInput {
        membership_id: MembershipId::new("789"),
        membership_type: MembershipType::new(3),
        checked_at: NOW,
        source: SourceMarker::Profile,
        definition_version: DefinitionVersion::new("v1"),
        characters: vec![character("456")],
        items: vec![ItemInstance {
            instance_id: ItemInstanceId::new("123"),
            item_hash: ItemHash::new(1),
            location,
            quantity: 1,
            equipped,
            locked: false,
            power: None,
            stats: vec![],
            plugs: vec![],
            perks: vec![],
            available_plugs: vec![],
        }],
        plug_set_entries: vec![],
    }
}

type Mutations = MutationService<QueuedProfileGateway, SqliteStore, DeterministicPlanIds>;

fn mutations(
    initial: InventorySnapshotInput,
    post: Option<Result<InventorySnapshotInput, AppError>>,
) -> Mutations {
    mutations_with_preflight(initial.clone(), initial, post)
}

fn mutations_with_preflight(
    initial: InventorySnapshotInput,
    preflight: InventorySnapshotInput,
    post: Option<Result<InventorySnapshotInput, AppError>>,
) -> Mutations {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions: vec![ItemDefinition {
                    hash: ItemHash::new(1),
                    bucket_hash: Some(BucketHash::new(1498876634)),
                    name: "Ace of Spades".into(),
                    item_type: Some("Hand Cannon".into()),
                    tier: Some("Exotic".into()),
                    element: None,
                    slot: Some(ItemSlot::Kinetic),
                    class_type: Some("any".parse().unwrap()),
                    plug_category: None,
                    tags: vec![],
                    version: DefinitionVersion::new("v1"),
                }],
            },
            checked_at: NOW,
            source: SourceMarker::Manifest,
        })
        .unwrap();
    store.replace_current(initial).unwrap();
    let mut gateway = QueuedProfileGateway::new();
    gateway.queue_membership(Ok(membership()));
    gateway.queue_inventory(Ok(preflight));
    if let Some(post) = post {
        gateway.queue_membership(Ok(membership()));
        gateway.queue_inventory(post);
    }
    MutationService::new(
        ProfileSyncService::new_with_definitions(gateway, store),
        DeterministicPlanIds::starting_at(1),
    )
}

#[test]
fn cli_equip_routes_selectors_and_preserves_verified_outcomes_offline() {
    for (item, selector, source, already, equip_reply, post, expected) in [
        (
            "Ace of Spades",
            "HuNtEr",
            ItemLocation::Vault,
            false,
            EquipOutcome::Equipped,
            Some(Ok(snapshot(target(), true))),
            EquipStatus::VerifiedSuccess,
        ),
        (
            "123",
            "456",
            target(),
            true,
            EquipOutcome::Equipped,
            None,
            EquipStatus::VerifiedNoop,
        ),
        (
            "123",
            "hunter",
            ItemLocation::Vault,
            false,
            EquipOutcome::Rejected(EquipRejection::ExoticLimit),
            Some(Ok(snapshot(target(), false))),
            EquipStatus::Incomplete,
        ),
        (
            "123",
            "Hunter",
            ItemLocation::Vault,
            false,
            EquipOutcome::Uncertain,
            Some(Ok(snapshot(target(), false))),
            EquipStatus::Incomplete,
        ),
        (
            "123",
            "Hunter",
            ItemLocation::Vault,
            false,
            EquipOutcome::Uncertain,
            Some(Ok(snapshot(target(), true))),
            EquipStatus::VerifiedSuccess,
        ),
        (
            "123",
            "Hunter",
            ItemLocation::Vault,
            false,
            EquipOutcome::Equipped,
            Some(Err(AppError::new(ErrorCategory::ProviderUnavailable))),
            EquipStatus::VerificationFailed,
        ),
        (
            "123",
            "Hunter",
            ItemLocation::Vault,
            false,
            EquipOutcome::Equipped,
            Some(Ok(snapshot(ItemLocation::Vault, false))),
            EquipStatus::VerificationFailed,
        ),
    ] {
        let mut service = mutations(snapshot(source, already), post);
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transferred));
        let mut equip = QueuedEquipGateway::new();
        equip.queue(Ok(equip_reply));
        let output = equip_item(
            &EquipArgs {
                item: item.into(),
                character: selector.into(),
            },
            &mut service,
            &mut transfer,
            &mut equip,
            &FakeClock::fixed(NOW),
            POLICY,
        )
        .unwrap();
        assert_eq!(output.status, expected, "{}", output.render_text());
        assert_eq!(output.json()["item"]["name"], "Ace of Spades");
        assert_eq!(output.json()["character"]["label"], "Hunter");
        if already {
            assert!(transfer.requests().is_empty());
            assert!(equip.requests().is_empty());
            assert_eq!(output.profile_refresh, ProfileRefresh::NotRequired);
        } else {
            assert_eq!(transfer.requests().len(), 1);
            assert_eq!(equip.requests().len(), 1);
        }
        if output.profile_refresh == ProfileRefresh::Failed {
            assert!(
                output.observed.is_none(),
                "must not label stale state as refreshed"
            );
        }
        if expected == EquipStatus::Incomplete {
            assert!(!output.observed.as_ref().unwrap().equipped);
            assert_eq!(output.steps[0].outcome, "completed");
            assert_eq!(output.exit_code(), 41);
        }
    }
}

#[test]
fn cli_equip_refuses_ambiguity_missing_and_invalid_without_dispatch() {
    for (item, target, duplicate_item, duplicate_character, reason) in [
        ("Ace", "Hunter", true, false, "use the instance id"),
        ("123", "Hunter", false, true, "use the character id"),
        ("999", "Hunter", false, false, "item was not found"),
        ("123", "missing", false, false, "character was not found"),
    ] {
        let mut initial = snapshot(ItemLocation::Vault, false);
        if duplicate_item {
            let mut second = initial.items[0].clone();
            second.instance_id = ItemInstanceId::new("124");
            initial.items.push(second);
        }
        if duplicate_character {
            initial.characters.push(character("457"));
        }
        let mut service = mutations(initial, None);
        let mut transfer = QueuedTransferGateway::new();
        let mut equip = QueuedEquipGateway::new();
        let output = equip_item(
            &EquipArgs {
                item: item.into(),
                character: target.into(),
            },
            &mut service,
            &mut transfer,
            &mut equip,
            &FakeClock::fixed(NOW),
            POLICY,
        )
        .unwrap();
        assert_eq!(output.status, EquipStatus::Refused);
        assert!(output.render_text().contains(reason));
        assert!(transfer.requests().is_empty());
        assert!(equip.requests().is_empty());
        assert_ne!(output.exit_code(), 0);
    }
}

#[test]
fn cli_equip_stops_after_transfer_failure_and_reports_unrun_equip() {
    for reply in [
        Ok(TransferOutcome::Rejected(
            TransferRejection::DestinationFull,
        )),
        Ok(TransferOutcome::Transient),
        Ok(TransferOutcome::Uncertain),
        Err(AppError::new(ErrorCategory::ProviderUnavailable)),
    ] {
        let dispatched = reply.is_ok();
        let post = dispatched.then(|| Ok(snapshot(ItemLocation::Vault, false)));
        let mut service = mutations(snapshot(ItemLocation::Vault, false), post);
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(reply);
        let mut equip = QueuedEquipGateway::new();
        let output = equip_item(
            &EquipArgs {
                item: "123".into(),
                character: "Hunter".into(),
            },
            &mut service,
            &mut transfer,
            &mut equip,
            &FakeClock::fixed(NOW),
            POLICY,
        )
        .unwrap();
        assert!(equip.requests().is_empty());
        assert_eq!(output.steps[1].outcome, "not_run");
        if dispatched {
            assert_eq!(output.status, EquipStatus::Incomplete);
            assert_eq!(output.profile_refresh, ProfileRefresh::Refreshed);
            assert!(output.render_text().contains("profile refreshed"));
        } else {
            assert_eq!(output.status, EquipStatus::Refused);
            assert_eq!(output.profile_refresh, ProfileRefresh::NotRequired);
            assert_eq!(output.exit_code(), 21);
        }
    }
}

#[test]
fn cli_equip_invalid_arguments_stop_before_composition_or_clock() {
    struct NeverClock;
    impl Clock for NeverClock {
        fn now(&self) -> Result<Timestamp, AppError> {
            panic!("invalid arguments must not reach the clock")
        }
    }
    for (item, character) in [
        (" ", "Hunter"),
        ("Ace\n", "Hunter"),
        ("123", " "),
        ("123", "id\n"),
    ] {
        let args = EquipArgs {
            item: item.into(),
            character: character.into(),
        };
        let mut service = mutations(snapshot(ItemLocation::Vault, false), None);
        let error = equip_item(
            &args,
            &mut service,
            &mut QueuedTransferGateway::new(),
            &mut QueuedEquipGateway::new(),
            &NeverClock,
            POLICY,
        )
        .unwrap_err();
        assert_eq!(error.exit_code(), 2);
        let output = destiny_cli::run_binary([
            "ghostlink",
            "equip",
            item,
            "--character",
            character,
            "--json",
        ]);
        assert_eq!(output.exit_code, 2);
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn cli_equip_preflight_change_refuses_before_transfer() {
    let mut service = mutations_with_preflight(
        snapshot(ItemLocation::Vault, false),
        snapshot(target(), false),
        None,
    );
    let mut transfer = QueuedTransferGateway::new();
    let mut equip = QueuedEquipGateway::new();
    let output = equip_item(
        &EquipArgs {
            item: "123".into(),
            character: "Hunter".into(),
        },
        &mut service,
        &mut transfer,
        &mut equip,
        &FakeClock::fixed(NOW),
        POLICY,
    )
    .unwrap();
    assert_eq!(output.status, EquipStatus::Refused);
    assert!(transfer.requests().is_empty());
    assert!(equip.requests().is_empty());
    assert!(output.steps.iter().all(|step| step.outcome == "not_run"));
}
