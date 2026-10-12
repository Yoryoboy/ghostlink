//! Executor behavior with fakes only. Every gateway records its calls into one shared
//! log and refuses any dispatch the test did not script.
use destiny_adapters::sqlite::SqliteStore;
use destiny_app::build_apply_exec::*;
use destiny_core::build_apply::*;
use destiny_core::domain::ItemSlot;
use destiny_core::*;
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

const KINETIC: i64 = 1498876634;
const ENERGY: i64 = 2465295065;

fn definition(hash: i64, bucket: i64) -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(hash),
        bucket_hash: Some(BucketHash::new(bucket)),
        name: format!("Item {hash}"),
        item_type: Some("Weapon".into()),
        tier: Some("Legendary".into()),
        element: None,
        slot: ItemSlot::from_equipment_bucket(BucketHash::new(bucket)),
        class_type: Some(domain::ClassType::Any),
        plug_category: None,
        tags: vec![],
        version: DefinitionVersion::new("v1"),
        armor: ArmorDefinition::default(),
        non_transferrable: Some(false),
        does_postmaster_pull_have_side_effects: Some(false),
        is_instance_item: Some(true),
        max_stack_size: Some(1),
        stack_unique_label: None,
    }
}
fn instance(id: &str, hash: i64, location: ItemLocation, equipped: bool) -> ItemInstance {
    ItemInstance {
        instance_id: ItemInstanceId::new(id),
        item_hash: ItemHash::new(hash),
        location,
        quantity: 1,
        equipped,
        locked: false,
        power: None,
        stats: vec![],
        perks: vec![],
        plugs: vec![],
        energy: None,
        gear_tier: None,
        sockets: vec![],
        transfer_status: None,
        available_plugs: vec![],
    }
}
fn c(id: &str) -> CharacterId {
    CharacterId::new(id)
}
fn on(id: &str) -> ItemLocation {
    ItemLocation::Character(c(id))
}
/// Items: 1 kinetic in Vault; 2 energy on character 2; 3/4 equipped on character 1;
/// 5 kinetic in character 1's Postmaster.
fn store() -> (SqliteStore, SyncRevision) {
    let mut repository = SqliteStore::open_in_memory().unwrap();
    repository
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions: vec![
                    definition(100, KINETIC),
                    definition(101, KINETIC),
                    definition(200, ENERGY),
                    definition(201, ENERGY),
                ],
            },
            checked_at: Timestamp::new(100),
            source: SourceMarker::Manifest,
        })
        .unwrap();
    let characters = ["1", "2"]
        .map(|id| Character {
            id: c(id),
            class: Some("Hunter".into()),
            name: None,
            race: None,
            gender: None,
            light: None,
            date_last_played: None,
        })
        .to_vec();
    let input = InventorySnapshotInput {
        membership_id: MembershipId::new("99"),
        membership_type: MembershipType::new(3),
        definition_version: DefinitionVersion::new("v1"),
        checked_at: Timestamp::new(100),
        source: SourceMarker::Profile,
        characters,
        items: vec![
            instance("1", 100, ItemLocation::Vault, false),
            instance("2", 200, on("2"), false),
            instance("3", 101, on("1"), true),
            instance("4", 201, on("1"), true),
            instance("5", 100, ItemLocation::Postmaster(c("1")), false),
        ],
        profile_currencies_coverage: ProfileCurrenciesCoverage::NotObserved,
        stacks: vec![],
        plug_set_entries: vec![],
    };
    repository
        .replace_current_profile(input, None, None)
        .unwrap();
    let revision = repository
        .load_current()
        .unwrap()
        .unwrap()
        .metadata
        .revision;
    (repository, revision)
}
fn mv(item: &str, from: ItemLocation, to: ItemLocation, needs: &[usize]) -> BuildApplyStep {
    BuildApplyStep {
        phase: ApplyPhase::Move,
        action: BuildAction::Move {
            item: ItemInstanceId::new(item),
            from,
            to,
        },
        needs: needs.to_vec(),
    }
}
fn equip(items: &[&str], character: &str) -> BuildApplyStep {
    BuildApplyStep {
        phase: ApplyPhase::Equip,
        action: BuildAction::Equip {
            items: items.iter().map(|i| ItemInstanceId::new(*i)).collect(),
            character: c(character),
        },
        needs: vec![],
    }
}
fn plan(steps: Vec<BuildApplyStep>) -> BuildApplyPlan {
    BuildApplyPlan {
        target: c("1"),
        steps,
        skipped: vec![],
        already_there: vec![],
        clear_skipped: vec![],
        not_applied: vec!["sockets_mods_subclass_deferred_to_be_007b"],
        unknown: vec![],
    }
}
/// Vault -> 1, then 2 via the Vault, then a grouped equip of both.
fn standard_plan() -> BuildApplyPlan {
    plan(vec![
        mv("1", ItemLocation::Vault, on("1"), &[]),
        mv("2", on("2"), ItemLocation::Vault, &[]),
        mv("2", ItemLocation::Vault, on("1"), &[1]),
        equip(&["1", "2"], "1"),
    ])
}

fn place(location: &ItemLocation) -> String {
    match location {
        ItemLocation::Vault => "vault".into(),
        ItemLocation::Character(id) => format!("c{}", id.as_str()),
        _ => "other".into(),
    }
}
type Log = Rc<RefCell<Vec<String>>>;
#[derive(Default)]
struct Script {
    transfer: VecDeque<Result<TransferOutcome, AppError>>,
    equip: VecDeque<Result<EquipOutcome, AppError>>,
    batch: VecDeque<Result<EquipItemsOutcome, AppError>>,
    pull: VecDeque<Result<InstancedPullOutcome, AppError>>,
}
struct Fake {
    log: Log,
    script: Rc<RefCell<Script>>,
}
fn pop<T>(queue: &mut VecDeque<T>, log: &Log, what: &str) -> T {
    match queue.pop_front() {
        Some(next) => next,
        None => {
            log.borrow_mut().push(format!("UNEXPECTED {what}"));
            panic!("unexpected mutation: {what}");
        }
    }
}
struct FakeTransfer(Fake);
struct FakeEquip(Fake);
struct FakeBatch(Fake);
struct FakePull(Fake);
impl TransferGateway for FakeTransfer {
    fn transfer(&mut self, r: &TransferRequest) -> Result<TransferOutcome, AppError> {
        let what = format!(
            "transfer {} {}->{}",
            r.item().as_str(),
            place(r.from()),
            place(r.to())
        );
        self.0.log.borrow_mut().push(what.clone());
        pop(&mut self.0.script.borrow_mut().transfer, &self.0.log, &what)
    }
}
impl EquipGateway for FakeEquip {
    fn equip(&mut self, r: &EquipItemRequest) -> Result<EquipOutcome, AppError> {
        let what = format!("equip {} on {}", r.item().as_str(), r.character().as_str());
        self.0.log.borrow_mut().push(what.clone());
        pop(&mut self.0.script.borrow_mut().equip, &self.0.log, &what)
    }
}
impl EquipItemsGateway for FakeBatch {
    fn equip_items(&mut self, r: &EquipItemsRequest) -> Result<EquipItemsOutcome, AppError> {
        let ids: Vec<_> = r.items().iter().map(|i| i.as_str()).collect();
        let what = format!(
            "equip_items {} on {}",
            ids.join(","),
            r.character().as_str()
        );
        self.0.log.borrow_mut().push(what.clone());
        pop(&mut self.0.script.borrow_mut().batch, &self.0.log, &what)
    }
}
impl InstancedPullGateway for FakePull {
    fn pull(&mut self, r: &InstancedPullRequest) -> Result<InstancedPullOutcome, AppError> {
        let what = format!("pull {} on {}", r.item().as_str(), r.character().as_str());
        self.0.log.borrow_mut().push(what.clone());
        pop(&mut self.0.script.borrow_mut().pull, &self.0.log, &what)
    }
}
type Gateways = BuildGateways<FakeTransfer, FakeEquip, FakeBatch, FakePull>;
fn gateways(script: Script) -> (Gateways, Log) {
    let log = Log::default();
    let script = Rc::new(RefCell::new(script));
    let fake = || Fake {
        log: log.clone(),
        script: script.clone(),
    };
    (
        BuildGateways {
            transfer: FakeTransfer(fake()),
            equip: FakeEquip(fake()),
            equip_items: FakeBatch(fake()),
            pull: FakePull(fake()),
        },
        log,
    )
}
fn transferred(n: usize) -> VecDeque<Result<TransferOutcome, AppError>> {
    (0..n).map(|_| Ok(TransferOutcome::Transferred)).collect()
}
fn ok_batch(ids: &[(&str, EquipItemStatus)]) -> Result<EquipItemsOutcome, AppError> {
    Ok(EquipItemsOutcome::Acknowledged(
        ids.iter()
            .map(|(id, status)| EquipItemResult {
                item: ItemInstanceId::new(*id),
                status: *status,
            })
            .collect(),
    ))
}
fn run(
    repository: &mut SqliteStore,
    revision: SyncRevision,
    plan: &BuildApplyPlan,
    script: Script,
) -> (PlanExecution, Vec<String>) {
    let (mut gateways, log) = gateways(script);
    let execution = execute_plan(repository, plan, revision, &mut gateways, &mut || {});
    let calls = log.borrow().clone();
    (execution, calls)
}
fn statuses(execution: &PlanExecution) -> Vec<Vec<ItemStatus>> {
    execution
        .steps
        .iter()
        .map(|s| s.outcomes.iter().map(|o| o.status.clone()).collect())
        .collect()
}
fn find(repository: &SqliteStore, id: &str) -> ItemInstance {
    repository
        .load_current()
        .unwrap()
        .unwrap()
        .items
        .into_iter()
        .find(|i| i.instance_id.as_str() == id)
        .unwrap()
}

#[test]
fn executes_steps_in_plan_order_and_updates_the_projection_like_other_actions() {
    let (mut repository, revision) = store();
    let script = Script {
        transfer: transferred(3),
        batch: [ok_batch(&[
            ("1", EquipItemStatus::Success),
            ("2", EquipItemStatus::Success),
        ])]
        .into(),
        ..Default::default()
    };
    let (execution, calls) = run(&mut repository, revision, &standard_plan(), script);
    assert_eq!(
        calls,
        [
            "transfer 1 vault->c1",
            "transfer 2 c2->vault",
            "transfer 2 vault->c1",
            "equip_items 1,2 on 1",
        ]
    );
    assert_eq!(execution.status, ApplyStatus::Applied);
    assert_eq!(execution.local_state, ApplyLocalState::Updated);
    assert!(
        execution
            .steps
            .iter()
            .all(|s| s.outcomes.iter().all(|o| o.status == ItemStatus::Succeeded))
    );
    // Intended results, no post-action read: moved, equipped, and replaced items unequipped.
    for id in ["1", "2"] {
        let item = find(&repository, id);
        assert_eq!((item.location, item.equipped), (on("1"), true), "{id}");
    }
    assert!(!find(&repository, "3").equipped);
    assert!(!find(&repository, "4").equipped);
}

#[test]
fn failed_step_skips_only_its_dependents_and_equips_what_succeeded() {
    // DIM: a failed move does not abort the loadout; items that did arrive are still
    // bulk-equipped (loadout-apply.ts:546-552, 615-626).
    let (mut repository, revision) = store();
    let script = Script {
        transfer: [
            Ok(TransferOutcome::Transferred),
            Ok(TransferOutcome::Rejected(
                TransferRejection::NotTransferable,
            )),
        ]
        .into(),
        batch: [ok_batch(&[("1", EquipItemStatus::Success)])].into(),
        ..Default::default()
    };
    let (execution, calls) = run(&mut repository, revision, &standard_plan(), script);
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert_eq!(calls[2], "equip_items 1 on 1");
    assert_eq!(
        statuses(&execution),
        [
            vec![ItemStatus::Succeeded],
            vec![ItemStatus::Failed(StepFailure::Rejected(
                "not_transferable"
            ))],
            vec![ItemStatus::Skipped(SkipCause::DependencyFailed(1))],
            vec![
                ItemStatus::Succeeded,
                ItemStatus::Skipped(SkipCause::DependencyFailed(2))
            ],
        ]
    );
    assert_eq!(execution.status, ApplyStatus::Partial);
    assert_eq!(execution.local_state, ApplyLocalState::RefreshPending);
    assert_eq!(
        (
            find(&repository, "1").location,
            find(&repository, "1").equipped
        ),
        (on("1"), true)
    );
    assert_eq!(find(&repository, "2").location, on("2"));
}

#[test]
fn batch_equip_reports_each_item_bungie_code_and_persists_only_successes() {
    let (mut repository, revision) = store();
    let script = Script {
        transfer: transferred(3),
        batch: [ok_batch(&[
            ("1", EquipItemStatus::Success),
            ("2", EquipItemStatus::Failure(1641)),
        ])]
        .into(),
        ..Default::default()
    };
    let (execution, _) = run(&mut repository, revision, &standard_plan(), script);
    assert_eq!(
        statuses(&execution)[3],
        [
            ItemStatus::Succeeded,
            ItemStatus::Failed(StepFailure::BungieCode(1641))
        ]
    );
    assert_eq!(execution.status, ApplyStatus::Partial);
    assert!(find(&repository, "1").equipped);
    assert!(!find(&repository, "2").equipped);
    assert!(!find(&repository, "3").equipped && find(&repository, "4").equipped);
}

#[test]
fn single_equip_uses_the_single_equip_gateway_and_rejections_are_typed() {
    let (mut repository, revision) = store();
    let one = plan(vec![equip(&["3"], "1")]);
    let script = Script {
        equip: [Ok(EquipOutcome::Rejected(EquipRejection::ExoticLimit))].into(),
        ..Default::default()
    };
    let (execution, calls) = run(&mut repository, revision, &one, script);
    assert_eq!(calls, ["equip 3 on 1"]);
    assert_eq!(
        statuses(&execution),
        [vec![ItemStatus::Failed(StepFailure::Rejected(
            "exotic_limit"
        ))]]
    );
    assert_eq!(execution.status, ApplyStatus::Failed);
    assert_eq!(execution.local_state, ApplyLocalState::RefreshPending);
}

#[test]
fn uncertain_outcome_is_never_retried_and_only_its_dependents_wait() {
    let (mut repository, revision) = store();
    let script = Script {
        transfer: [
            Ok(TransferOutcome::Uncertain),
            Ok(TransferOutcome::Transferred),
        ]
        .into(),
        ..Default::default()
    };
    let p = plan(vec![
        mv("1", ItemLocation::Vault, on("1"), &[]),
        mv("2", on("2"), ItemLocation::Vault, &[]),
        equip(&["1"], "1"),
    ]);
    let (execution, calls) = run(&mut repository, revision, &p, script);
    assert_eq!(
        calls.len(),
        2,
        "no retry, dependent equip not sent: {calls:?}"
    );
    assert_eq!(
        statuses(&execution),
        [
            vec![ItemStatus::Failed(StepFailure::Uncertain)],
            vec![ItemStatus::Succeeded],
            vec![ItemStatus::Skipped(SkipCause::DependencyFailed(0))],
        ]
    );
    // An uncertain move is not projected as applied.
    assert_eq!(find(&repository, "1").location, ItemLocation::Vault);
    assert_eq!(execution.local_state, ApplyLocalState::RefreshPending);
}

#[test]
fn throttle_fail_fast_stops_the_plan_without_further_dispatch() {
    let (mut repository, revision) = store();
    let script = Script {
        transfer: [Err(AppError::new(ErrorCategory::ProviderRateLimit))].into(),
        ..Default::default()
    };
    let (execution, calls) = run(&mut repository, revision, &standard_plan(), script);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(
        statuses(&execution)[0],
        [ItemStatus::Failed(StepFailure::NotDispatched(
            ErrorCategory::ProviderRateLimit
        ))]
    );
    assert!(
        statuses(&execution)[1..]
            .iter()
            .flatten()
            .all(|s| *s == ItemStatus::Skipped(SkipCause::ProviderStopped))
    );
    // Nothing reached Bungie: the category exit applies and local state is untouched.
    assert_eq!(
        execution.status,
        ApplyStatus::Refused(ErrorCategory::ProviderRateLimit)
    );
    assert_eq!(execution.local_state, ApplyLocalState::Unchanged);
    assert_eq!(find(&repository, "1").location, ItemLocation::Vault);
}

#[test]
fn throttle_after_progress_keeps_acknowledged_effects_and_reports_partial() {
    let (mut repository, revision) = store();
    let script = Script {
        transfer: [
            Ok(TransferOutcome::Transferred),
            Err(AppError::new(ErrorCategory::ProviderRateLimit)),
        ]
        .into(),
        ..Default::default()
    };
    let (execution, calls) = run(&mut repository, revision, &standard_plan(), script);
    assert_eq!(calls.len(), 2);
    assert_eq!(execution.status, ApplyStatus::Partial);
    assert_eq!(execution.local_state, ApplyLocalState::RefreshPending);
    assert_eq!(find(&repository, "1").location, on("1"));
}

#[test]
fn postmaster_pull_uses_the_instanced_pull_gateway_and_moves_the_item_home() {
    let (mut repository, revision) = store();
    let p = plan(vec![BuildApplyStep {
        phase: ApplyPhase::Move,
        action: BuildAction::PullFromPostmaster {
            item: ItemInstanceId::new("5"),
            character: c("1"),
        },
        needs: vec![],
    }]);
    let script = Script {
        pull: [Ok(InstancedPullOutcome::Acknowledged)].into(),
        ..Default::default()
    };
    let (execution, calls) = run(&mut repository, revision, &p, script);
    assert_eq!(calls, ["pull 5 on 1"]);
    assert_eq!(execution.status, ApplyStatus::Applied);
    assert_eq!(find(&repository, "5").location, on("1"));
}

#[test]
fn stale_projection_is_refused_before_any_dispatch() {
    let (mut repository, revision) = store();
    let (execution, calls) = run(
        &mut repository,
        SyncRevision::new(revision.value() + 1),
        &standard_plan(),
        Script::default(),
    );
    assert!(calls.is_empty());
    assert_eq!(
        execution.status,
        ApplyStatus::Refused(ErrorCategory::StaleState)
    );
    assert_eq!(execution.local_state, ApplyLocalState::Unchanged);
}

#[test]
fn empty_plan_is_a_no_op_without_dispatch() {
    let (mut repository, revision) = store();
    let (execution, calls) = run(&mut repository, revision, &plan(vec![]), Script::default());
    assert!(calls.is_empty());
    assert_eq!(execution.status, ApplyStatus::NoOp);
    assert_eq!(execution.local_state, ApplyLocalState::Unchanged);
}

#[test]
fn a_failed_space_making_move_skips_the_move_that_needed_the_room() {
    // The planner records that item 1's move relies on item 2 leaving first; the two
    // share no item, so only the recorded dependency can cascade (DIM: a failed
    // move-aside fails the move that required it, item-move-service.ts:677-741).
    let (mut repository, revision) = store();
    let p = plan(vec![
        mv("2", on("2"), ItemLocation::Vault, &[]),
        mv("1", ItemLocation::Vault, on("1"), &[0]),
    ]);
    let script = Script {
        transfer: [Ok(TransferOutcome::Rejected(
            TransferRejection::DestinationFull,
        ))]
        .into(),
        ..Default::default()
    };
    let (execution, calls) = run(&mut repository, revision, &p, script);
    assert_eq!(calls, ["transfer 2 c2->vault"]);
    assert_eq!(
        statuses(&execution)[1],
        [ItemStatus::Skipped(SkipCause::DependencyFailed(0))]
    );
    assert_eq!(find(&repository, "1").location, ItemLocation::Vault);
}
