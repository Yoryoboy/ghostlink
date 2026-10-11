//! Offline parser-to-runner genuine-instance pull journeys. No credential/HTTP use.
use destiny_adapters::equipment_capacity_policy::{
    PINNED_EQUIPMENT_BUCKET_PATH, PINNED_EQUIPMENT_CAPACITY_VERSION,
    pinned_equipment_capacity_policy,
};
use destiny_app::test_support::{
    DeterministicPlanIds, FakeClock, InMemoryDefinitionRepository, InMemoryInventoryRepository,
    QueuedInstancedPullGateway, QueuedManifestGateway, QueuedProfileGateway,
};
use destiny_app::*;
use destiny_cli::{
    pull::pull_item,
    runner::{parse_process, run_pull_with},
};
use std::{cell::RefCell, rc::Rc};

const NOW: Timestamp = Timestamp::new(100);
const FRESHNESS: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };
const BUCKET: i64 = 1498876634;
const ITEM: &str = "9223372036854775807";
fn version() -> DefinitionVersion {
    DefinitionVersion::new(PINNED_EQUIPMENT_CAPACITY_VERSION)
}
fn membership() -> DestinyMembership {
    DestinyMembership::new(MembershipType::new(3), MembershipId::new("123")).unwrap()
}
fn postmaster() -> ItemLocation {
    ItemLocation::Postmaster(CharacterId::new("2"))
}
fn arrival() -> ItemLocation {
    ItemLocation::Character(CharacterId::new("2"))
}
fn input(location: ItemLocation, checked_at: u64) -> InventorySnapshotInput {
    InventorySnapshotInput {
        membership_id: MembershipId::new("123"),
        membership_type: MembershipType::new(3),
        definition_version: version(),
        checked_at: Timestamp::new(checked_at),
        source: SourceMarker::Profile,
        characters: vec![Character {
            id: CharacterId::new("2"),
            name: None,
            class: None,
            race: None,
            gender: None,
            light: None,
            date_last_played: None,
        }],
        items: vec![ItemInstance {
            instance_id: ItemInstanceId::new(ITEM),
            item_hash: ItemHash::new(7),
            location,
            quantity: 1,
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
        stacks: vec![],
        plug_set_entries: vec![],
        profile_currencies_coverage: Default::default(),
    }
}
fn acquisition(input: InventorySnapshotInput) -> ProfileAcquisition {
    let rows = input
        .items
        .iter()
        .map(|item| {
            InstancedBucketObservation::new(
                item.instance_id.clone(),
                BucketHash::new(if matches!(item.location, ItemLocation::Postmaster(_)) {
                    215593132
                } else {
                    BUCKET
                }),
                item.location.clone(),
            )
            .unwrap()
        })
        .collect();
    ProfileAcquisition::new(input, Some(rows)).unwrap()
}
#[derive(Clone)]
struct Profile(Rc<RefCell<QueuedProfileGateway>>);
impl ProfileGateway for Profile {
    fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
        self.0.borrow_mut().resolve_membership()
    }
    fn resolve_membership_scoped(
        &mut self,
        reason: ProfileSyncReason,
    ) -> Result<DestinyMembership, AppError> {
        self.0.borrow_mut().resolve_membership_scoped(reason)
    }
    fn load_inventory(
        &mut self,
        membership: &DestinyMembership,
    ) -> Result<InventorySnapshotInput, AppError> {
        self.0.borrow_mut().load_inventory(membership)
    }
    fn load_inventory_with_observations_scoped(
        &mut self,
        membership: &DestinyMembership,
        reason: ProfileSyncReason,
    ) -> Result<ProfileAcquisition, AppError> {
        self.0
            .borrow_mut()
            .load_inventory_with_observations_scoped(membership, reason)
    }
}
type Service = InstancedPullService<
    Profile,
    InMemoryInventoryRepository,
    InMemoryDefinitionRepository,
    QueuedManifestGateway,
    FakeClock,
    DeterministicPlanIds,
>;
struct Fixture {
    service: Service,
    profile: Profile,
    action: QueuedInstancedPullGateway,
    constructions: usize,
}
impl Fixture {
    fn new(
        pre: InventorySnapshotInput,
        execution_pre: InventorySnapshotInput,
        posts: Vec<Result<ProfileAcquisition, AppError>>,
        receipt: Result<InstancedPullOutcome, AppError>,
        safe: Option<bool>,
        evidence_version: DefinitionVersion,
    ) -> Self {
        Self::with_repository(
            pre,
            execution_pre,
            posts,
            receipt,
            safe,
            evidence_version,
            InMemoryInventoryRepository::new(),
        )
    }
    fn with_repository(
        pre: InventorySnapshotInput,
        execution_pre: InventorySnapshotInput,
        posts: Vec<Result<ProfileAcquisition, AppError>>,
        receipt: Result<InstancedPullOutcome, AppError>,
        safe: Option<bool>,
        evidence_version: DefinitionVersion,
        mut repository: InMemoryInventoryRepository,
    ) -> Self {
        if repository.load_current().unwrap().is_none() {
            repository.replace_current(pre.clone()).unwrap();
        }
        let mut profile = QueuedProfileGateway::new();
        // Production resolves authenticated context once before planning. Every
        // fresh sync resolves it again, never deriving it from command input.
        profile.queue_membership(Ok(membership()));
        for result in [Ok(acquisition(pre)), Ok(acquisition(execution_pre))]
            .into_iter()
            .chain(posts)
        {
            profile.queue_membership(Ok(membership()));
            profile.queue_acquisition(result);
        }
        let profile = Profile(Rc::new(RefCell::new(profile)));
        let definition = ItemDefinition {
            hash: ItemHash::new(7),
            bucket_hash: Some(BucketHash::new(BUCKET)),
            non_transferrable: None,
            does_postmaster_pull_have_side_effects: safe,
            is_instance_item: Some(true),
            max_stack_size: None,
            stack_unique_label: None,
            name: "PRIVATE_NAME\n\u{1b}secret".into(),
            item_type: None,
            tier: None,
            element: None,
            slot: None,
            class_type: None,
            plug_category: None,
            tags: vec![],
            version: version(),
        };
        let mut manifest = QueuedManifestGateway::new();
        for _ in 0..2 {
            manifest.queue_capacity_evidence(Ok(EquipmentCapacityEvidence::new(
                evidence_version.clone(),
                PINNED_EQUIPMENT_BUCKET_PATH,
            )
            .unwrap()));
        }
        let service = InstancedPullService::new(
            ProfileSyncService::new(profile.clone(), repository),
            InMemoryDefinitionRepository::new(version(), vec![definition]),
            manifest,
            pinned_equipment_capacity_policy(),
            FakeClock::fixed(NOW),
            DeterministicPlanIds::starting_at(1),
        );
        let mut action = QueuedInstancedPullGateway::new();
        action.queue(receipt);
        Self {
            service,
            profile,
            action,
            constructions: 0,
        }
    }
    fn standard(
        posts: Vec<Result<ProfileAcquisition, AppError>>,
        receipt: Result<InstancedPullOutcome, AppError>,
    ) -> Self {
        Self::new(
            input(postmaster(), 100),
            input(postmaster(), 100),
            posts,
            receipt,
            Some(false),
            version(),
        )
    }
    fn run(&mut self, args: &[&str]) -> destiny_cli::ProcessOutput {
        run_pull_with(args.iter().copied(), |item| {
            self.constructions += 1;
            let membership = self
                .profile
                .resolve_membership_scoped(ProfileSyncReason::Preflight)?;
            Ok(pull_item(
                item,
                membership,
                &mut self.service,
                &mut self.action,
                FRESHNESS,
            ))
        })
    }
    fn reasons(&self) -> Vec<ProfileSyncReason> {
        self.profile.0.borrow().requested_reasons().to_vec()
    }
}
fn body(output: &destiny_cli::ProcessOutput) -> serde_json::Value {
    serde_json::from_str(if output.exit_code == 0 {
        &output.stdout
    } else {
        &output.stderr
    })
    .unwrap()
}
#[test]
fn acknowledged_pull_persistence_failure_exits_42_and_retains_receipt() {
    let mut fixture = Fixture::with_repository(
        input(postmaster(), 100),
        input(postmaster(), 100),
        vec![],
        Ok(InstancedPullOutcome::Acknowledged),
        Some(false),
        version(),
        InMemoryInventoryRepository::with_revision_for_test(
            input(postmaster(), 100),
            SyncRevision::new(u64::MAX),
        ),
    );
    let output = fixture.run(&["ghostlink", "pull", ITEM, "--json"]);
    assert_eq!(output.exit_code, 42);
    assert!(output.stdout.is_empty());
    let value = body(&output);
    assert_eq!(value["disposition"], "local_state_failed");
    assert_eq!(value["receipt"]["kind"], "acknowledged");
    assert_eq!(value["failure"]["category"], "local_storage");
    assert!(
        value["failure"]["message"]
            .as_str()
            .unwrap()
            .contains("local state could not be updated after the action")
    );
    assert_eq!(fixture.action.calls(), 1);
    assert!(fixture.reasons().is_empty());
}

#[test]
fn canonical_pull_command_is_accepted() {
    assert!(parse_process(["ghostlink", "pull", ITEM, "--json"]).is_ok());
}
#[test]
fn invalid_selection_has_zero_auth_service_and_action_work() {
    for item in [
        "0",
        "-1",
        "9223372036854775808",
        "01",
        "+1",
        " 1",
        "1 ",
        "1.0",
        "Ace of Spades",
        "hash:7",
        "",
        "\nPRIVATE",
    ] {
        let mut fixture = Fixture::standard(vec![], Ok(InstancedPullOutcome::Acknowledged));
        let output = fixture.run(&["ghostlink", "--json", "pull", item]);
        assert_eq!(output.exit_code, 2, "{item:?}");
        assert_eq!(body(&output)["error"]["code"], "invalid_request");
        assert_eq!(fixture.constructions, 0);
        assert_eq!(fixture.profile.0.borrow().membership_calls(), 0);
        assert_eq!(fixture.profile.0.borrow().observed_acquisition_calls(), 0);
        assert_eq!(fixture.action.calls(), 0);
        assert!(!output.stderr.contains("PRIVATE"));
    }
    for flag in [
        "--quantity",
        "--destination",
        "--owner",
        "--character",
        "--item-hash",
        "--vault",
    ] {
        let output = run_pull_with(["ghostlink", "pull", "1", flag, "2"], |_| {
            panic!("no runtime")
        });
        assert_eq!(output.exit_code, 2);
    }
}
#[test]
fn acknowledged_stale_profile_propagates_applied_and_only_one_action() {
    let mut fixture = Fixture::standard(
        vec![Ok(acquisition(input(postmaster(), 100)))],
        Ok(InstancedPullOutcome::Acknowledged),
    );
    let output = fixture.run(&["ghostlink", "pull", ITEM, "--json"]);
    assert_eq!(output.exit_code, 0);
    assert!(output.stderr.is_empty());
    let value = body(&output);
    assert_eq!(value["disposition"], "applied");
    assert_eq!(value["receipt"]["kind"], "acknowledged");
    assert!(value.get("observations").is_none());
    assert_eq!(value["dispatch"]["item"]["instance_id"], ITEM);
    assert_eq!(value["dispatch"]["destination"]["character_id"], "2");
    assert_eq!(fixture.action.calls(), 1);
    assert_eq!(fixture.action.requests()[0].item().as_str(), ITEM);
    assert_eq!(fixture.action.requests()[0].item_hash().value(), 7);
    assert!(fixture.reasons().is_empty());
    assert_eq!(fixture.profile.0.borrow().membership_calls(), 1);
}
#[test]
fn eligibility_capacity_and_preflight_drift_refuse_without_dispatch_or_observation() {
    let mut full = input(postmaster(), 100);
    for id in 10..20 {
        let mut item = full.items[0].clone();
        item.instance_id = ItemInstanceId::new(id.to_string());
        item.location = arrival();
        full.items.push(item);
    }
    for (pre, execution_pre, safe, evidence, stage, reason) in [
        (
            input(arrival(), 100),
            input(postmaster(), 100),
            Some(false),
            version(),
            "planning",
            "not_in_postmaster",
        ),
        (
            input(postmaster(), 100),
            input(postmaster(), 100),
            None,
            version(),
            "planning",
            "side_effects_unknown",
        ),
        (
            input(postmaster(), 100),
            input(postmaster(), 100),
            Some(true),
            version(),
            "planning",
            "side_effects",
        ),
        (
            full,
            input(postmaster(), 100),
            Some(false),
            version(),
            "planning",
            "capacity",
        ),
    ] {
        let mut fixture = Fixture::new(
            pre,
            execution_pre,
            vec![],
            Ok(InstancedPullOutcome::Acknowledged),
            safe,
            evidence,
        );
        let output = fixture.run(&["ghostlink", "pull", ITEM, "--json"]);
        assert_ne!(output.exit_code, 0);
        let value = body(&output);
        assert_eq!(value["failure"]["stage"], stage);
        assert_eq!(value["failure"]["reason"], reason);
        assert!(value["receipt"].is_null());
        assert!(value.get("observations").is_none());
        assert_eq!(fixture.action.calls(), 0);
        assert!(!fixture.reasons().contains(&ProfileSyncReason::PostAction));
    }
}
#[test]
fn changed_manifest_capacity_evidence_and_unread_provider_drift_do_not_gate_pull() {
    let mut fixture = Fixture::new(
        input(postmaster(), 100),
        input(arrival(), 100),
        vec![],
        Ok(InstancedPullOutcome::Acknowledged),
        Some(false),
        DefinitionVersion::new("changed"),
    );
    assert_eq!(
        fixture
            .run(&["ghostlink", "pull", ITEM, "--json"])
            .exit_code,
        0
    );
    assert_eq!(fixture.action.calls(), 1);
    assert!(fixture.reasons().is_empty());
}

#[test]
fn uncertain_and_refused_receipts_are_nonzero_without_observation() {
    for (receipt, disposition) in [
        (InstancedPullOutcome::Uncertain, "uncertain"),
        (
            InstancedPullOutcome::Rejected(InstancedPullRejection::InvalidRequest),
            "refused",
        ),
        (
            InstancedPullOutcome::Rejected(InstancedPullRejection::DestinationFull),
            "refused",
        ),
        (InstancedPullOutcome::AuthenticationRefused, "refused"),
        (InstancedPullOutcome::Transient, "refused"),
    ] {
        let mut fixture =
            Fixture::standard(vec![Ok(acquisition(input(arrival(), 100)))], Ok(receipt));
        let output = fixture.run(&["ghostlink", "pull", ITEM, "--json"]);
        assert_eq!(output.exit_code, 41);
        assert!(output.stdout.is_empty());
        assert_eq!(body(&output)["disposition"], disposition);
        assert!(!body(&output)["receipt"].is_null());
        assert!(body(&output).get("observations").is_none());
        assert_eq!(fixture.action.calls(), 1);
        assert!(fixture.reasons().is_empty());
    }
}
#[test]
fn help_is_pure_and_read_only_entry_rejects_pull() {
    use destiny_cli::command::Cli;
    for args in [
        vec!["ghostlink", "--help"],
        vec!["ghostlink", "pull", "--help"],
    ] {
        let help = run_pull_with(args, |_| panic!("help cannot construct runtime"));
        assert_eq!(help.exit_code, 0);
        assert!(help.stderr.is_empty());
        assert!(help.stdout.contains("pull"));
    }
    let cli = parse_process(["ghostlink", "pull", "1"]).unwrap();
    assert!(Cli::into_read_request(cli).is_err());
}
#[test]
fn typed_gateway_error_has_no_observations_or_receipt() {
    let mut fixture = Fixture::standard(
        vec![],
        Err(AppError::new(ErrorCategory::ProviderAuthentication)),
    );
    let output = fixture.run(&["ghostlink", "pull", ITEM, "--json"]);
    assert_eq!(output.exit_code, 11);
    let value = body(&output);
    assert_eq!(value["failure"]["stage"], "gateway");
    assert_eq!(value["failure"]["category"], "provider_authentication");
    assert!(value["receipt"].is_null());
    assert!(value.get("observations").is_none());
    assert_eq!(fixture.action.calls(), 1);
    assert!(fixture.reasons().is_empty());
}
#[test]
fn mismatch_pipeline_reports_snapshot_and_stored_versions_without_dispatch() {
    let mut pre = input(postmaster(), 100);
    pre.definition_version = DefinitionVersion::new("snapshot-other");
    let mut fixture = Fixture::new(
        pre.clone(),
        pre,
        vec![],
        Ok(InstancedPullOutcome::Acknowledged),
        Some(false),
        version(),
    );
    let output = fixture.run(&["ghostlink", "pull", ITEM]);
    assert_ne!(output.exit_code, 0);
    for expected in ["snapshot-other", version().as_str(), "ghostlink sync"] {
        assert!(output.stderr.contains(expected), "{}", output.stderr);
    }
    assert_eq!(fixture.action.calls(), 0);
}

#[test]
fn json_is_shared_mapper_exactly_and_text_retains_all_sanitized_evidence() {
    let posts = || {
        vec![
            Err(AppError::invalid_argument("test", "PRIVATE_PROVIDER_BODY")),
            Ok(acquisition(input(arrival(), 99))),
        ]
    };
    let mut expected_fixture = Fixture::standard(posts(), Ok(InstancedPullOutcome::Acknowledged));
    let context = expected_fixture.profile.resolve_membership().unwrap();
    let plan = expected_fixture
        .service
        .plan(
            &InstancedPullPlanRequest {
                membership: context,
                item: ItemInstanceId::new(ITEM),
            },
            PlanLifetime::new(60).unwrap(),
            FRESHNESS,
        )
        .unwrap();
    let outcome = expected_fixture
        .service
        .execute(plan, &mut expected_fixture.action, FRESHNESS)
        .unwrap();
    let shared = destiny_output::instanced_pull::InstancedPullOutput::from_outcome(&outcome);
    let mut fixture = Fixture::standard(posts(), Ok(InstancedPullOutcome::Acknowledged));
    assert_eq!(
        body(&fixture.run(&["ghostlink", "pull", ITEM, "--json"])),
        shared.json()
    );
    let mut fixture = Fixture::standard(posts(), Ok(InstancedPullOutcome::Acknowledged));
    let text = fixture.run(&["ghostlink", "pull", ITEM]);
    assert_eq!(text.exit_code, 0);
    for evidence in ["applied", "Bungie accepted the pull; not re-read."] {
        assert!(text.stdout.contains(evidence), "{evidence}");
    }
    assert!(!text.stdout.contains("PRIVATE"));
    assert!(!text.stdout.contains('\u{1b}'));
    assert!(text.stderr.is_empty());
}
