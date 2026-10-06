//! Deterministic CLI lock journeys through the shared service and SQLite.
use destiny_adapters::sqlite::SqliteStore;
use destiny_app::test_support::{
    DeterministicPlanIds, FakeClock, QueuedLockGateway, QueuedProfileGateway,
};
use destiny_app::*;
use destiny_cli::{command::LockArgs, lock::set_lock_state, lock_output::LockStatus};

const NOW: Timestamp = Timestamp::new(100);
const POLICY: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };
fn snapshot(locked: bool, vault: bool) -> InventorySnapshotInput {
    InventorySnapshotInput {
        profile_currencies_coverage: Default::default(),
        stacks: Vec::new(),
        membership_id: MembershipId::new("789"),
        membership_type: MembershipType::new(3),
        checked_at: NOW,
        source: SourceMarker::Profile,
        definition_version: DefinitionVersion::new("v1"),
        characters: vec![Character {
            id: CharacterId::new("456"),
            name: None,
            class: Some("Hunter".into()),
            race: None,
            gender: None,
            light: None,
            date_last_played: None,
        }],
        items: vec![ItemInstance {
            instance_id: ItemInstanceId::new("123"),
            item_hash: ItemHash::new(1),
            location: if vault {
                ItemLocation::Vault
            } else {
                ItemLocation::Character(CharacterId::new("456"))
            },
            quantity: 1,
            equipped: true,
            locked,
            power: None,
            stats: vec![],
            plugs: vec![],
            perks: vec![],
            available_plugs: vec![],
        }],
        plug_set_entries: vec![],
    }
}
fn service(
    initial: InventorySnapshotInput,
    pre: InventorySnapshotInput,
    post: Option<Result<InventorySnapshotInput, AppError>>,
) -> MutationService<QueuedProfileGateway, SqliteStore, DeterministicPlanIds> {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions: vec![ItemDefinition {
                    hash: ItemHash::new(1),
                    bucket_hash: None,
                    name: "Ace\nof Spades".into(),
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
    store.replace_current(initial).unwrap();
    let mut gateway = QueuedProfileGateway::new();
    for input in std::iter::once(Ok(pre)).chain(post) {
        gateway.queue_membership(Ok(DestinyMembership::new(
            MembershipType::new(3),
            MembershipId::new("789"),
        )
        .unwrap()));
        gateway.queue_inventory(input);
    }
    MutationService::new(
        ProfileSyncService::new_with_definitions(gateway, store),
        DeterministicPlanIds::starting_at(1),
    )
}
#[test]
fn lock_pipeline_covers_both_states_locations_and_all_terminal_outcomes() {
    for locked in [false, true] {
        for vault in [false, true] {
            for (reply, post, status, code) in [
                (
                    LockOutcome::Applied,
                    Ok(snapshot(locked, vault)),
                    LockStatus::VerifiedSuccess,
                    0,
                ),
                (
                    LockOutcome::Uncertain,
                    Ok(snapshot(locked, vault)),
                    LockStatus::VerifiedSuccess,
                    0,
                ),
                (
                    LockOutcome::Applied,
                    Ok(snapshot(!locked, vault)),
                    LockStatus::AppliedPendingProviderRefresh,
                    43,
                ),
                (
                    LockOutcome::Uncertain,
                    Ok(snapshot(!locked, vault)),
                    LockStatus::Incomplete,
                    41,
                ),
                (
                    LockOutcome::Rejected(LockRejection::NotInSocialSpace),
                    Ok(snapshot(!locked, vault)),
                    LockStatus::Incomplete,
                    41,
                ),
                (
                    LockOutcome::AuthenticationRefused,
                    Ok(snapshot(locked, vault)),
                    LockStatus::Incomplete,
                    41,
                ),
                (
                    LockOutcome::Applied,
                    Err(AppError::new(ErrorCategory::ProviderUnavailable)),
                    LockStatus::VerificationFailed,
                    42,
                ),
                (
                    LockOutcome::Applied,
                    Ok({
                        let mut changed = snapshot(!locked, vault);
                        changed.items[0].quantity = 2;
                        changed
                    }),
                    LockStatus::VerificationFailed,
                    42,
                ),
                (
                    LockOutcome::Applied,
                    Ok({
                        let mut missing = snapshot(!locked, vault);
                        missing.items.clear();
                        missing
                    }),
                    LockStatus::VerificationFailed,
                    42,
                ),
            ] {
                let initial = snapshot(!locked, vault);
                let mut mutations = service(initial.clone(), initial, Some(post));
                let mut gateway = QueuedLockGateway::new();
                gateway.queue(Ok(reply));
                let output = set_lock_state(
                    &LockArgs { item: "123".into() },
                    locked,
                    &mut mutations,
                    &mut gateway,
                    &FakeClock::fixed(NOW),
                    POLICY,
                )
                .unwrap();
                assert_eq!(output.status, status, "{}", output.render_text());
                assert_eq!(output.exit_code(), code);
                assert_eq!(gateway.requests().len(), 1);
                assert_eq!(gateway.requests()[0].character().as_str(), "456");
                assert_eq!(gateway.requests()[0].locked(), locked);
                assert_eq!(output.json()["item"]["name"], "Ace?of Spades");
                if output.profile_refresh == destiny_cli::lock_output::ProfileRefresh::Failed {
                    assert!(output.observed.is_none());
                }
                if code == 43 {
                    assert!(output.render_text().contains("do not retry"));
                }
                let process = destiny_cli::runner::lock_process_output(
                    destiny_cli::OutputMode::Json,
                    &output,
                );
                assert_eq!(process.exit_code, code);
                assert_eq!(process.stderr.is_empty(), code == 0);
                assert_eq!(output.render_text().contains("✔ verified"), code == 0);
            }
            let initial = snapshot(!locked, vault);
            let mut mutations = service(initial, snapshot(locked, vault), None);
            let mut gateway = QueuedLockGateway::new();
            let output = set_lock_state(
                &LockArgs { item: "123".into() },
                locked,
                &mut mutations,
                &mut gateway,
                &FakeClock::fixed(NOW),
                POLICY,
            )
            .unwrap();
            assert_eq!(output.status, LockStatus::VerifiedNoop);
            assert_eq!(output.exit_code(), 0);
            assert!(output.render_text().contains("already"));
            assert!(gateway.requests().is_empty());
        }
    }
}
#[test]
fn lock_pipeline_semantic_selection_and_invalid_args_before_clock() {
    let initial = snapshot(false, true);
    let mut mutations = service(initial.clone(), initial, Some(Ok(snapshot(true, true))));
    let mut gateway = QueuedLockGateway::new();
    gateway.queue(Ok(LockOutcome::Applied));
    let output = set_lock_state(
        &LockArgs { item: "Ace".into() },
        true,
        &mut mutations,
        &mut gateway,
        &FakeClock::fixed(NOW),
        POLICY,
    )
    .unwrap();
    assert_eq!(output.status, LockStatus::VerifiedSuccess);
    assert_eq!(gateway.requests()[0].item().as_str(), "123");

    struct NeverClock;
    impl Clock for NeverClock {
        fn now(&self) -> Result<Timestamp, AppError> {
            panic!("invalid selector reached clock")
        }
    }
    for item in ["".into(), "bad\nitem".into(), "x".repeat(257)] {
        assert!(
            set_lock_state(
                &LockArgs { item },
                true,
                &mut mutations,
                &mut gateway,
                &NeverClock,
                POLICY,
            )
            .is_err()
        );
    }
    assert_eq!(gateway.requests().len(), 1);
}

#[test]
fn lock_pipeline_refuses_missing_ambiguous_and_unknown_location() {
    for case in ["missing", "ambiguous", "unknown"] {
        let mut initial = snapshot(false, true);
        let item = match case {
            "missing" => "999",
            "ambiguous" => "Ace",
            _ => "123",
        };
        if case == "ambiguous" {
            let mut second = initial.items[0].clone();
            second.instance_id = ItemInstanceId::new("124");
            initial.items.push(second);
        }
        if case == "unknown" {
            initial.items[0].location = ItemLocation::Other("secret-sentinel".into());
        }
        let mut mutations = service(initial.clone(), initial, None);
        let mut gateway = QueuedLockGateway::new();
        let output = set_lock_state(
            &LockArgs { item: item.into() },
            true,
            &mut mutations,
            &mut gateway,
            &FakeClock::fixed(NOW),
            POLICY,
        )
        .unwrap();
        assert_eq!(output.status, LockStatus::Refused);
        assert!(gateway.requests().is_empty());
        assert!(!output.json().to_string().contains("secret-sentinel"));
    }
}
