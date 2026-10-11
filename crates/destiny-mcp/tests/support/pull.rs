use destiny_adapters::equipment_capacity_policy::{
    PINNED_EQUIPMENT_BUCKET_PATH, PINNED_EQUIPMENT_CAPACITY_VERSION,
    pinned_equipment_capacity_policy,
};
use destiny_app::test_support::{
    DeterministicPlanIds, FakeClock, InMemoryDefinitionRepository, InMemoryInventoryRepository,
    InstancedPullRequest, QueuedInstancedPullGateway, QueuedManifestGateway, QueuedProfileGateway,
};
use destiny_app::*;
use destiny_mcp::schema::PullItemArgs;
use destiny_mcp::service::{InstancedPullToolService, MutationToolService};
use std::{cell::RefCell, rc::Rc};

const ITEM: &str = "9223372036854775807";
const POLICY: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };
fn version() -> DefinitionVersion {
    DefinitionVersion::new(PINNED_EQUIPMENT_CAPACITY_VERSION)
}
fn membership() -> DestinyMembership {
    DestinyMembership::new(MembershipType::new(3), MembershipId::new("123")).unwrap()
}
fn snapshot(arrived: bool) -> ProfileAcquisition {
    let location = if arrived {
        ItemLocation::Character(CharacterId::new("2"))
    } else {
        ItemLocation::Postmaster(CharacterId::new("2"))
    };
    let rows = vec![
        InstancedBucketObservation::new(
            ItemInstanceId::new(ITEM),
            BucketHash::new(if arrived { 1498876634 } else { 215593132 }),
            location.clone(),
        )
        .unwrap(),
    ];
    ProfileAcquisition::new(
        InventorySnapshotInput {
            membership_id: MembershipId::new("123"),
            membership_type: MembershipType::new(3),
            definition_version: version(),
            checked_at: Timestamp::new(100),
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
        },
        Some(rows),
    )
    .unwrap()
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
        _: &DestinyMembership,
    ) -> Result<InventorySnapshotInput, AppError> {
        panic!("must preserve actual acquisitions")
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
#[derive(Clone)]
struct Action(Rc<RefCell<QueuedInstancedPullGateway>>);
impl InstancedPullGateway for Action {
    fn pull(&mut self, request: &InstancedPullRequest) -> Result<InstancedPullOutcome, AppError> {
        self.0.borrow_mut().pull(request)
    }
}
type Pipeline = InstancedPullService<
    Profile,
    InMemoryInventoryRepository,
    InMemoryDefinitionRepository,
    QueuedManifestGateway,
    FakeClock,
    DeterministicPlanIds,
>;
struct Fixture {
    pipeline: Pipeline,
    profile: Profile,
    action: Action,
}
impl Fixture {
    fn new(arrived: bool, receipt: InstancedPullOutcome, failed_observation: bool) -> Self {
        let mut profile = QueuedProfileGateway::new();
        profile.queue_membership(Ok(membership()));
        for acquisition in [Ok(snapshot(false)), Ok(snapshot(false))]
            .into_iter()
            .chain((0..3).map(|index| {
                if failed_observation && index == 0 {
                    Err(AppError::new(ErrorCategory::ProviderUnavailable))
                } else {
                    Ok(snapshot(arrived))
                }
            }))
        {
            profile.queue_membership(Ok(membership()));
            profile.queue_acquisition(acquisition);
        }
        let profile = Profile(Rc::new(RefCell::new(profile)));
        let definition = ItemDefinition {
            hash: ItemHash::new(7),
            bucket_hash: Some(BucketHash::new(1498876634)),
            non_transferrable: None,
            does_postmaster_pull_have_side_effects: Some(false),
            is_instance_item: Some(true),
            max_stack_size: None,
            stack_unique_label: None,
            name: "Private\nname".into(),
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
                version(),
                PINNED_EQUIPMENT_BUCKET_PATH,
            )
            .unwrap()));
        }
        let mut repository = InMemoryInventoryRepository::new();
        repository
            .replace_current(snapshot(false).input().clone())
            .unwrap();
        let pipeline = InstancedPullService::new(
            ProfileSyncService::new(profile.clone(), repository),
            InMemoryDefinitionRepository::new(version(), vec![definition]),
            manifest,
            pinned_equipment_capacity_policy(),
            FakeClock::fixed(Timestamp::new(100)),
            DeterministicPlanIds::starting_at(1),
        );
        let mut action = QueuedInstancedPullGateway::new();
        action.queue(Ok(receipt));
        Self {
            pipeline,
            profile,
            action: Action(Rc::new(RefCell::new(action))),
        }
    }
    fn mcp(self, item: &str) -> serde_json::Value {
        let action = self.action.clone();
        let profile = self.profile.clone();
        let mut service =
            InstancedPullToolService::new(self.profile, self.pipeline, self.action, POLICY);
        let value = service
            .pull_item(&PullItemArgs { item: item.into() })
            .unwrap()
            .json();
        assert_eq!(action.0.borrow().calls(), 1);
        let reasons = profile.0.borrow().requested_reasons().to_vec();
        assert!(reasons.is_empty());
        assert!(value.get("observations").is_none());
        value
    }
    fn cli(mut self) -> serde_json::Value {
        let membership = self.profile.resolve_membership().unwrap();
        destiny_cli::pull::pull_item(
            ItemInstanceId::new(ITEM),
            membership,
            &mut self.pipeline,
            &mut self.action,
            POLICY,
        )
        .json()
    }
}
pub fn parity() {
    for (arrived, receipt, failed, disposition) in [
        (true, InstancedPullOutcome::Acknowledged, false, "applied"),
        (true, InstancedPullOutcome::Uncertain, false, "uncertain"),
        (false, InstancedPullOutcome::Acknowledged, false, "applied"),
        (false, InstancedPullOutcome::Uncertain, false, "uncertain"),
        (
            true,
            InstancedPullOutcome::Rejected(InstancedPullRejection::InvalidRequest),
            false,
            "refused",
        ),
        (false, InstancedPullOutcome::Acknowledged, true, "applied"),
    ] {
        let cli = Fixture::new(arrived, receipt, failed).cli();
        let mcp = Fixture::new(arrived, receipt, failed).mcp(ITEM);
        assert_eq!(mcp, cli);
        assert_eq!(mcp["disposition"], disposition);
        assert!(mcp.get("observations").is_none());
    }
}
pub fn reject_before_io() {
    let fixture = Fixture::new(true, InstancedPullOutcome::Acknowledged, false);
    let profile = fixture.profile.clone();
    let action = fixture.action.clone();
    let mut service =
        InstancedPullToolService::new(fixture.profile, fixture.pipeline, fixture.action, POLICY);
    for item in ["0", "01", "9223372036854775808", "Ace"] {
        assert!(
            service
                .pull_item(&PullItemArgs { item: item.into() })
                .is_err()
        );
    }
    assert_eq!(profile.0.borrow().membership_calls(), 0);
    assert_eq!(profile.0.borrow().observed_acquisition_calls(), 0);
    assert_eq!(action.0.borrow().calls(), 0);
}
