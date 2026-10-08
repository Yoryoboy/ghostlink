use destiny_app::test_support::*;
use destiny_app::*;
use destiny_mcp::schema::PullStackArgs;
use destiny_mcp::service::{MutationToolService, StackPullToolService};
use std::{cell::RefCell, rc::Rc};
const POLICY: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };
fn version() -> DefinitionVersion {
    DefinitionVersion::new("stack-fixture")
}
fn membership() -> DestinyMembership {
    DestinyMembership::new(MembershipType::new(3), MembershipId::new("123")).unwrap()
}
fn snapshot(arrived: bool) -> ProfileAcquisition {
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
            items: vec![],
            stacks: vec![
                StackObservation::new(
                    ItemHash::new(7),
                    BucketHash::new(1469714392),
                    if arrived { 23 } else { 20 },
                    StackContainer::ProfileInventory,
                )
                .unwrap(),
                StackObservation::new(
                    ItemHash::new(7),
                    BucketHash::new(215593132),
                    if arrived { 7 } else { 10 },
                    StackContainer::Postmaster(CharacterId::new("2")),
                )
                .unwrap(),
            ],
            plug_set_entries: vec![],
            profile_currencies_coverage: Default::default(),
        },
        None,
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
        panic!("must preserve acquisitions")
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
struct Action(Rc<RefCell<QueuedStackPullGateway>>);
impl StackPullGateway for Action {
    fn pull(&mut self, request: &StackPullRequest) -> Result<StackPullOutcome, AppError> {
        self.0.borrow_mut().pull(request)
    }
}
struct Delay;
impl RetryDelay for Delay {
    fn wait(&mut self, duration: std::time::Duration) {
        assert_eq!(duration.as_secs(), 2);
    }
}
type Pipeline = StackPullService<
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
    fn new(arrived: bool, receipt: StackPullOutcome, failed: bool) -> Self {
        let mut profile = QueuedProfileGateway::new();
        profile.queue_membership(Ok(membership()));
        for acquisition in [Ok(snapshot(false)), Ok(snapshot(false))]
            .into_iter()
            .chain((0..3).map(|index| {
                if failed && index == 0 {
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
            bucket_hash: Some(BucketHash::new(1469714392)),
            does_postmaster_pull_have_side_effects: Some(false),
            is_instance_item: Some(false),
            max_stack_size: Some(100),
            stack_unique_label: Some(String::new()),
            name: "Stack".into(),
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
                "artifact",
            )
            .unwrap()));
        }
        let pipeline = StackPullService::new(
            ProfileSyncService::new(profile.clone(), InMemoryInventoryRepository::new()),
            InMemoryDefinitionRepository::new(version(), vec![definition]),
            manifest,
            StackBucketCapacityPolicy::new(
                version(),
                "artifact",
                vec![
                    (BucketHash::new(1469714392), 50, true),
                    (BucketHash::new(3313201758), 50, true),
                ],
            )
            .unwrap(),
            FakeClock::fixed(Timestamp::new(100)),
            DeterministicPlanIds::starting_at(1),
        );
        let mut action = QueuedStackPullGateway::new();
        action.queue(Ok(receipt));
        Self {
            pipeline,
            profile,
            action: Action(Rc::new(RefCell::new(action))),
        }
    }
    fn mcp(self) -> serde_json::Value {
        let action = self.action.clone();
        let profile = self.profile.clone();
        let mut service =
            StackPullToolService::new(self.profile, self.pipeline, self.action, Delay, POLICY);
        let output = service
            .pull_stack(&PullStackArgs {
                item_hash: "7".into(),
                amount: "3".into(),
                character: "2".into(),
            })
            .unwrap()
            .json();
        assert_eq!(action.0.borrow().calls(), 1);
        let reasons = profile.0.borrow().requested_reasons().to_vec();
        assert_eq!(
            &reasons[..2],
            &[ProfileSyncReason::Preflight, ProfileSyncReason::Preflight]
        );
        assert!(
            reasons[2..]
                .iter()
                .all(|r| *r == ProfileSyncReason::PostAction)
        );
        output
    }
    fn cli(mut self) -> serde_json::Value {
        let membership = self.profile.resolve_membership().unwrap();
        destiny_cli::pull_stack::pull_stack(
            destiny_cli::command::StackPullSelection {
                item_hash: ItemHash::new(7),
                amount: 3,
                owner: CharacterId::new("2"),
            },
            membership,
            &mut self.pipeline,
            &mut self.action,
            &mut Delay,
            POLICY,
        )
        .json()
    }
}
pub fn parity() {
    for (arrived, receipt, failed, disposition) in [
        (true, StackPullOutcome::Acknowledged, false, "verified"),
        (true, StackPullOutcome::Uncertain, false, "verified"),
        (false, StackPullOutcome::Acknowledged, false, "uncertain"),
        (false, StackPullOutcome::Uncertain, false, "uncertain"),
        (
            true,
            StackPullOutcome::Rejected(StackPullRejection::InvalidRequest),
            false,
            "refused",
        ),
        (false, StackPullOutcome::Acknowledged, true, "uncertain"),
    ] {
        let cli = Fixture::new(arrived, receipt, failed).cli();
        let mcp = Fixture::new(arrived, receipt, failed).mcp();
        assert_eq!(mcp, cli);
        assert_eq!(mcp["disposition"], disposition);
    }
}
pub fn reject_before_io() {
    let f = Fixture::new(true, StackPullOutcome::Acknowledged, false);
    let profile = f.profile.clone();
    let action = f.action.clone();
    let mut service = StackPullToolService::new(f.profile, f.pipeline, f.action, Delay, POLICY);
    for (hash, amount, character) in [("01", "3", "2"), ("7", "0", "2"), ("7", "3", "0")] {
        assert!(
            service
                .pull_stack(&PullStackArgs {
                    item_hash: hash.into(),
                    amount: amount.into(),
                    character: character.into()
                })
                .is_err()
        );
    }
    assert_eq!(profile.0.borrow().membership_calls(), 0);
    assert_eq!(profile.0.borrow().observed_acquisition_calls(), 0);
    assert_eq!(action.0.borrow().calls(), 0);
}
