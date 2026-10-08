//! Offline move journeys through the real shared planner/executor and SQLite.
use destiny_adapters::sqlite::SqliteStore;
use destiny_app::test_support::{
    DeterministicPlanIds, FakeClock, QueuedProfileGateway, QueuedTransferGateway,
};
use destiny_app::*;
use destiny_cli::command::MoveArgs;
use destiny_cli::move_item::move_item;
use destiny_cli::move_output::{MoveStatus, ProfileRefresh};

const NOW: Timestamp = Timestamp::new(100);
const POLICY: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };
fn target() -> ItemLocation {
    ItemLocation::Character(CharacterId::new("456"))
}
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
fn snapshot(location: ItemLocation) -> InventorySnapshotInput {
    InventorySnapshotInput {
        profile_currencies_coverage: Default::default(),
        stacks: Vec::new(),
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
            equipped: false,
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
fn mutations(
    initial: InventorySnapshotInput,
    preflight: InventorySnapshotInput,
    post: Option<Result<InventorySnapshotInput, AppError>>,
) -> MutationService<QueuedProfileGateway, SqliteStore, DeterministicPlanIds> {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions: vec![ItemDefinition {
                    does_postmaster_pull_have_side_effects: None,
                    is_instance_item: None,
                    max_stack_size: None,
                    stack_unique_label: None,
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
    for inventory in std::iter::once(Ok(preflight)).chain(post) {
        gateway.queue_membership(Ok(DestinyMembership::new(
            MembershipType::new(3),
            MembershipId::new("789"),
        )
        .unwrap()));
        gateway.queue_inventory(inventory);
    }
    MutationService::new(
        ProfileSyncService::new_with_definitions(gateway, store),
        DeterministicPlanIds::starting_at(1),
    )
}
fn args(vault: bool) -> MoveArgs {
    MoveArgs {
        item: "123".into(),
        character: (!vault).then(|| "HuNtEr".into()),
        vault,
    }
}
#[test]
fn cli_move_verified_outcomes_and_no_retry() {
    for vault in [false, true] {
        let source = if vault { target() } else { ItemLocation::Vault };
        let destination = if vault { ItemLocation::Vault } else { target() };
        for (reply, post, status, code) in [
            (
                TransferOutcome::Transferred,
                Ok(snapshot(destination.clone())),
                MoveStatus::Applied,
                0,
            ),
            (
                TransferOutcome::Uncertain,
                Ok(snapshot(destination.clone())),
                MoveStatus::Incomplete,
                41,
            ),
            (
                TransferOutcome::Rejected(TransferRejection::DestinationFull),
                Ok(snapshot(source.clone())),
                MoveStatus::Incomplete,
                41,
            ),
            (
                TransferOutcome::Uncertain,
                Ok(snapshot(source.clone())),
                MoveStatus::Incomplete,
                41,
            ),
            (
                TransferOutcome::Transferred,
                Ok(snapshot(source.clone())),
                MoveStatus::Applied,
                0,
            ),
            (
                TransferOutcome::Transferred,
                Err(AppError::new(ErrorCategory::ProviderUnavailable)),
                MoveStatus::Applied,
                0,
            ),
            (
                TransferOutcome::Transferred,
                Ok(snapshot(ItemLocation::Character(CharacterId::new(
                    "elsewhere",
                )))),
                MoveStatus::Applied,
                0,
            ),
        ] {
            let initial = snapshot(source.clone());
            let mut service = mutations(initial.clone(), initial, Some(post));
            let mut transfer = QueuedTransferGateway::new();
            transfer.queue(Ok(reply));
            let mut request = args(vault);
            request.item = "Ace of Spades".into();
            let output = move_item(
                &request,
                &mut service,
                &mut transfer,
                &FakeClock::fixed(NOW),
                POLICY,
            )
            .unwrap();
            assert_eq!(output.status, status, "{}", output.render_text());
            assert_eq!(output.exit_code(), code);
            assert_eq!(transfer.requests().len(), 1);
            assert_eq!(output.json()["item"]["name"], "Ace of Spades");
            assert_eq!(output.steps.len(), 1);
            if output.profile_refresh == ProfileRefresh::Failed {
                assert!(output.observed.is_none());
            }
            assert_eq!(output.profile_refresh, ProfileRefresh::NotRequired);
            assert!(output.observed.is_none());
            if status == MoveStatus::Applied {
                assert!(output.render_text().contains("not re-read"));
                assert!(!output.render_text().contains("✔ verified"));
            }
        }
        let initial = snapshot(destination);
        let mut service = mutations(initial.clone(), initial, None);
        let mut transfer = QueuedTransferGateway::new();
        let output = move_item(
            &args(vault),
            &mut service,
            &mut transfer,
            &FakeClock::fixed(NOW),
            POLICY,
        )
        .unwrap();
        assert_eq!(output.status, MoveStatus::Refused);
        assert_eq!(output.exit_code(), 40);
        let label = if vault { "Vault" } else { "Hunter" };
        assert!(
            output
                .render_text()
                .contains(&format!("already in {label}"))
        );
        assert!(
            output.json()["reason"]
                .as_str()
                .unwrap()
                .contains(&format!("already in {label}"))
        );
        assert!(transfer.requests().is_empty());
    }
}
#[test]
fn cli_move_refuses_ambiguity_and_missing_selectors_without_dispatch() {
    for case in [
        "item",
        "character",
        "missing_item",
        "missing_character",
        "exact",
    ] {
        let mut initial = snapshot(ItemLocation::Vault);
        let mut request = args(false);
        match case {
            "item" => {
                request.item = "Ace".into();
                let mut second = initial.items[0].clone();
                second.instance_id = ItemInstanceId::new("124");
                initial.items.push(second);
            }
            "character" => initial.characters.push(character("457")),
            "missing_item" => request.item = "999".into(),
            "missing_character" => request.character = Some("missing".into()),
            "exact" => {
                initial.characters.push(character("457"));
                request.character = Some("456".into());
                initial.items[0].location = target();
            }
            _ => {}
        }
        let mut service = mutations(initial.clone(), initial, None);
        let mut transfer = QueuedTransferGateway::new();
        let output = move_item(
            &request,
            &mut service,
            &mut transfer,
            &FakeClock::fixed(NOW),
            POLICY,
        )
        .unwrap();
        assert_eq!(
            output.status,
            MoveStatus::Refused,
            "{case}: {}",
            output.render_text()
        );
        assert_eq!(
            output.exit_code(),
            match case {
                "item" | "character" => 31,
                "missing_item" => 32,
                "missing_character" => 2,
                "exact" => 40,
                _ => unreachable!(),
            }
        );
        assert!(transfer.requests().is_empty());
    }
}
#[test]
fn cli_move_local_revision_drift_refuses_before_dispatch() {
    let initial = snapshot(ItemLocation::Vault);
    let mut service = mutations(initial.clone(), initial.clone(), None);
    let request = MoveRequest {
        membership: MembershipId::new("789"),
        item: ItemSelector::instance(ItemInstanceId::new("123")),
        destination: DestinationSelector::character(CharacterId::new("456")),
        intent: MoveIntentAuthorization::Explicit,
    };
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, POLICY, NOW)
        .unwrap();
    let mut changed = initial;
    changed.items[0].locked = true;
    service.repository_mut().replace_current(changed).unwrap();
    let mut transfer = QueuedTransferGateway::new();
    let failure = service
        .execute_move_action(&mut transfer, &request, &plan, POLICY, NOW, NOW)
        .unwrap_err();
    assert_eq!(failure.error(), MutationError::StaleRevision);
    let output = destiny_cli::move_output::MoveOutput::refused(failure.error())
        .with_evidence(failure.evidence(), &[]);
    assert_eq!(output.status, MoveStatus::Refused);
    assert_eq!(output.exit_code(), 30);
    assert!(output.steps.iter().all(|step| step.outcome == "not_run"));
    assert!(transfer.requests().is_empty());
}

#[test]
fn cli_move_committed_local_destination_is_verified_without_dispatch() {
    for vault in [false, true] {
        let source = if vault { target() } else { ItemLocation::Vault };
        let destination = if vault { ItemLocation::Vault } else { target() };
        let mut service = mutations(snapshot(source.clone()), snapshot(source), None);
        let request = MoveRequest {
            membership: MembershipId::new("789"),
            item: ItemSelector::instance(ItemInstanceId::new("123")),
            destination: if vault {
                DestinationSelector::vault()
            } else {
                DestinationSelector::character(CharacterId::new("456"))
            },
            intent: MoveIntentAuthorization::Explicit,
        };
        let plan = service
            .plan_move(&request, PlanLifetime::MAX, POLICY, NOW)
            .unwrap();
        // A local publication supersedes the source with the requested end state.
        let committed = service
            .repository_mut()
            .replace_current(snapshot(destination))
            .unwrap();
        let mut transfer = QueuedTransferGateway::new();
        let outcome = service
            .execute_move_action(&mut transfer, &request, &plan, POLICY, NOW, NOW)
            .unwrap();
        let output = destiny_cli::move_output::MoveOutput::from_outcome(
            &outcome,
            &committed.characters,
            committed.items.first(),
        );
        assert_eq!(output.status, MoveStatus::VerifiedNoop);
        assert_eq!(output.exit_code(), 0);
        assert_eq!(output.profile_refresh, ProfileRefresh::NotRequired);
        assert_eq!(output.steps[0].outcome, "not_run");
        assert_eq!(output.json()["verification"], "verified");
        assert!(transfer.requests().is_empty());
    }
}

#[test]
fn cli_move_equipped_item_is_a_provider_refusal_after_one_dispatch() {
    let mut initial = snapshot(target());
    initial.items[0].equipped = true;
    let mut service = mutations(initial.clone(), initial.clone(), Some(Ok(initial)));
    let mut transfer = QueuedTransferGateway::new();
    transfer.queue(Ok(TransferOutcome::Rejected(TransferRejection::Equipped)));
    let output = move_item(
        &args(true),
        &mut service,
        &mut transfer,
        &FakeClock::fixed(NOW),
        POLICY,
    )
    .unwrap();
    assert_eq!(output.status, MoveStatus::Incomplete);
    assert_eq!(output.exit_code(), 41);
    assert_eq!(transfer.requests().len(), 1);
    assert_eq!(output.steps[0].outcome, "rejected");
    assert!(output.render_text().contains("item is equipped"));
    assert_eq!(output.profile_refresh, ProfileRefresh::NotRequired);
}

#[test]
fn cli_move_character_route_stops_at_failed_step_without_rollback() {
    for reply in [
        TransferOutcome::Transferred,
        TransferOutcome::Rejected(TransferRejection::DestinationFull),
        TransferOutcome::Uncertain,
    ] {
        let mut initial = snapshot(ItemLocation::Character(CharacterId::new("457")));
        initial.characters.push(character("457"));
        let success = reply == TransferOutcome::Transferred;
        let mut post = initial.clone();
        post.items[0].location = if success {
            target()
        } else {
            ItemLocation::Vault
        };
        let mut service = mutations(initial.clone(), initial, Some(Ok(post)));
        let mut transfer = QueuedTransferGateway::new();
        transfer.queue(Ok(TransferOutcome::Transferred));
        transfer.queue(Ok(reply));
        let request = MoveArgs {
            item: "123".into(),
            character: Some("456".into()),
            vault: false,
        };
        let output = move_item(
            &request,
            &mut service,
            &mut transfer,
            &FakeClock::fixed(NOW),
            POLICY,
        )
        .unwrap();
        assert_eq!(
            output.status,
            if success {
                MoveStatus::Applied
            } else {
                MoveStatus::Incomplete
            }
        );
        assert_eq!(transfer.requests().len(), 2);
        assert_eq!(
            output.steps[0]
                .from
                .as_ref()
                .unwrap()
                .character_id
                .as_deref(),
            Some("457")
        );
        assert_eq!(output.steps[0].to.kind, "vault");
        assert_eq!(output.steps[1].from.as_ref().unwrap().kind, "vault");
        assert_eq!(output.steps[1].to.character_id.as_deref(), Some("456"));
        assert_eq!(
            output.json()["steps_completed"],
            if success { 2 } else { 1 }
        );
        assert_eq!(
            output.json()["steps_uncompleted"],
            if success { 0 } else { 1 }
        );
        assert!(output.json()["observed"].is_null());
    }
    let mut initial = snapshot(ItemLocation::Character(CharacterId::new("457")));
    initial.characters.push(character("457"));
    let mut service = mutations(initial.clone(), initial.clone(), Some(Ok(initial)));
    let mut transfer = QueuedTransferGateway::new();
    transfer.queue(Ok(TransferOutcome::Rejected(
        TransferRejection::DestinationFull,
    )));
    let request = MoveArgs {
        item: "123".into(),
        character: Some("456".into()),
        vault: false,
    };
    let output = move_item(
        &request,
        &mut service,
        &mut transfer,
        &FakeClock::fixed(NOW),
        POLICY,
    )
    .unwrap();
    assert_eq!(output.status, MoveStatus::Incomplete);
    assert_eq!(transfer.requests().len(), 1);
    assert_eq!(output.steps[1].outcome, "not_run");
}

#[test]
fn cli_move_invalid_intent_stops_before_clock() {
    struct NeverClock;
    impl Clock for NeverClock {
        fn now(&self) -> Result<Timestamp, AppError> {
            panic!("invalid arguments reached clock")
        }
    }
    for request in [
        MoveArgs {
            item: "123".into(),
            character: None,
            vault: false,
        },
        MoveArgs {
            item: "123".into(),
            character: Some("Hunter".into()),
            vault: true,
        },
        MoveArgs {
            item: "Ace\n".into(),
            character: None,
            vault: true,
        },
        MoveArgs {
            item: "123".into(),
            character: Some(" ".into()),
            vault: false,
        },
    ] {
        let initial = snapshot(ItemLocation::Vault);
        let mut service = mutations(initial.clone(), initial, None);
        assert_eq!(
            move_item(
                &request,
                &mut service,
                &mut QueuedTransferGateway::new(),
                &NeverClock,
                POLICY
            )
            .unwrap_err()
            .exit_code(),
            2
        );
    }
}
