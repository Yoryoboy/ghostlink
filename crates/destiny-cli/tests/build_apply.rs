//! `build apply` through the CLI composition seam with fakes only. Mutation gateways
//! record every call and panic when the test did not script it.
use destiny_adapters::{dim_loadout::DimBuildCodec, sqlite::SqliteStore};
use destiny_app::build_apply_exec::BuildGateways;
use destiny_app::builds::{BuildSelector, BuildService, LocalBuildRequest};
use destiny_app::domain::{ClassType, ItemSlot};
use destiny_app::*;
use destiny_cli::command::{BuildApplyArgs, BuildSelection};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

const KINETIC: i64 = 1498876634;

fn definition(hash: i64) -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(hash),
        bucket_hash: Some(BucketHash::new(KINETIC)),
        name: format!("Item {hash}"),
        item_type: Some("Weapon".into()),
        tier: Some("Legendary".into()),
        element: None,
        slot: ItemSlot::from_equipment_bucket(BucketHash::new(KINETIC)),
        class_type: Some(ClassType::Any),
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
struct Profile(InventorySnapshotInput);
impl ProfileGateway for Profile {
    fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
        DestinyMembership::new(self.0.membership_type, self.0.membership_id.clone())
    }
    fn load_inventory(
        &mut self,
        _: &DestinyMembership,
    ) -> Result<InventorySnapshotInput, AppError> {
        Ok(self.0.clone())
    }
    fn load_inventory_profile_scoped(
        &mut self,
        _: &DestinyMembership,
        _: ProfileSyncReason,
    ) -> Result<ProfileInventoryRead, AppError> {
        Ok((self.0.clone(), None, Some(vec![])))
    }
}
type Sync = ProfileSyncService<Profile, SqliteStore>;
type Service = BuildService<destiny_app::test_support::InMemoryBuildRepository, DimBuildCodec>;
const FRESH: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };
fn now() -> Timestamp {
    Timestamp::new(100)
}
/// Item 1 sits in the Vault, item 3 is equipped on the only Hunter; the build wants 1.
fn fixture() -> (Service, Sync, String) {
    fixture_in(SqliteStore::open_in_memory().unwrap())
}
fn profile_input(with_item_1: bool) -> InventorySnapshotInput {
    let characters = [("1", "Hunter"), ("2", "Titan")]
        .map(|(id, class)| Character {
            id: CharacterId::new(id),
            class: Some(class.into()),
            name: None,
            race: None,
            gender: None,
            light: None,
            date_last_played: None,
        })
        .to_vec();
    InventorySnapshotInput {
        membership_id: MembershipId::new("99"),
        membership_type: MembershipType::new(3),
        definition_version: DefinitionVersion::new("v1"),
        checked_at: now(),
        source: SourceMarker::Profile,
        characters,
        items: vec![
            instance("1", 100, ItemLocation::Vault, false),
            instance(
                "3",
                101,
                ItemLocation::Character(CharacterId::new("1")),
                true,
            ),
        ]
        .into_iter()
        .filter(|i| with_item_1 || i.instance_id.as_str() != "1")
        .collect(),
        profile_currencies_coverage: ProfileCurrenciesCoverage::NotObserved,
        stacks: vec![],
        plug_set_entries: vec![],
    }
}
fn fixture_in(mut repository: SqliteStore) -> (Service, Sync, String) {
    let input = profile_input(true);
    repository
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions: vec![definition(100), definition(101)],
            },
            checked_at: now(),
            source: SourceMarker::Manifest,
        })
        .unwrap();
    repository
        .replace_current_profile(input.clone(), None, Some(vec![]))
        .unwrap();
    let mut service = BuildService::new(
        destiny_app::test_support::InMemoryBuildRepository::default(),
        DimBuildCodec,
    );
    let membership =
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("99")).unwrap();
    service
        .execute_local(
            &membership,
            &LocalBuildRequest::Import(
                r#"{"id":"b1","name":"Raid","classType":1,"equipped":[{"id":"1","hash":100}],"unequipped":[]}"#
                    .into(),
            ),
            now(),
        )
        .unwrap();
    let id = service
        .show(&membership, &BuildSelector::Name("Raid".into()))
        .unwrap()
        .definition
        .id;
    (
        service,
        ProfileSyncService::new_with_definitions(Profile(input), repository),
        id,
    )
}
fn args(id: &str, dry_run: bool) -> BuildApplyArgs {
    BuildApplyArgs {
        selection: BuildSelection {
            id: Some(id.into()),
            name: None,
        },
        character: None,
        dry_run,
    }
}

#[derive(Default)]
struct Script {
    transfer: VecDeque<Result<TransferOutcome, AppError>>,
    equip: VecDeque<Result<EquipOutcome, AppError>>,
    /// Runs while a transfer is in flight, before Bungie's answer is returned.
    during_transfer: Option<Box<dyn Fn()>>,
}
struct Recorder {
    log: Rc<RefCell<Vec<String>>>,
    script: Rc<RefCell<Script>>,
}
impl TransferGateway for Recorder {
    fn transfer(&mut self, r: &TransferRequest) -> Result<TransferOutcome, AppError> {
        self.log
            .borrow_mut()
            .push(format!("transfer {}", r.item().as_str()));
        if let Some(hook) = &self.script.borrow().during_transfer {
            hook();
        }
        self.script
            .borrow_mut()
            .transfer
            .pop_front()
            .expect("unexpected mutation: transfer")
    }
}
impl EquipGateway for Recorder {
    fn equip(&mut self, r: &EquipItemRequest) -> Result<EquipOutcome, AppError> {
        self.log
            .borrow_mut()
            .push(format!("equip {}", r.item().as_str()));
        self.script
            .borrow_mut()
            .equip
            .pop_front()
            .expect("unexpected mutation: equip")
    }
}
impl EquipItemsGateway for Recorder {
    fn equip_items(&mut self, _: &EquipItemsRequest) -> Result<EquipItemsOutcome, AppError> {
        panic!("unexpected mutation: equip_items")
    }
}
impl InstancedPullGateway for Recorder {
    fn pull(&mut self, _: &InstancedPullRequest) -> Result<InstancedPullOutcome, AppError> {
        panic!("unexpected mutation: pull")
    }
}
impl InsertPlugGateway for Recorder {
    fn insert_plug(&mut self, _: &InsertPlugRequest) -> Result<InsertPlugOutcome, AppError> {
        panic!("unexpected mutation: insert_plug")
    }
}
type Gateways = BuildGateways<Recorder, Recorder, Recorder, Recorder, Recorder>;
fn gateways(script: Script) -> (Gateways, Rc<RefCell<Vec<String>>>) {
    let log = Rc::new(RefCell::new(vec![]));
    let script = Rc::new(RefCell::new(script));
    let recorder = || Recorder {
        log: log.clone(),
        script: script.clone(),
    };
    (
        BuildGateways {
            transfer: recorder(),
            equip: recorder(),
            equip_items: recorder(),
            pull: recorder(),
            socket: recorder(),
        },
        log,
    )
}
fn run(
    fixture: &mut (Service, Sync, String),
    dry_run: bool,
    gateways: Option<&mut Gateways>,
) -> destiny_output::builds::BuildOutput {
    let (service, sync, id) = fixture;
    destiny_cli::builds::apply_build(
        service,
        sync,
        &args(id, dry_run),
        gateways,
        FRESH,
        now(),
        &mut || {},
    )
    .unwrap()
}
fn item(fixture: &(Service, Sync, String), id: &str) -> ItemInstance {
    fixture
        .1
        .repository()
        .load_current()
        .unwrap()
        .unwrap()
        .items
        .into_iter()
        .find(|i| i.instance_id.as_str() == id)
        .unwrap()
}

#[test]
fn dry_run_dispatches_nothing_even_with_gateways_present() {
    let mut f = fixture();
    let (mut gateways, log) = gateways(Script::default());
    let output = run(&mut f, true, Some(&mut gateways));
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.value["kind"], "build_apply_plan");
    assert_eq!(output.value["dry_run"], true);
    assert_eq!(output.value["dispatched"], 0);
    assert_eq!(output.value["steps"].as_array().unwrap().len(), 2);
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
    assert_eq!(item(&f, "1").location, ItemLocation::Vault);
}

#[test]
fn apply_dispatches_in_plan_order_and_updates_local_state() {
    let mut f = fixture();
    let (mut gateways, log) = gateways(Script {
        transfer: [Ok(TransferOutcome::Transferred)].into(),
        equip: [Ok(EquipOutcome::Equipped)].into(),
        ..Default::default()
    });
    let output = run(&mut f, false, Some(&mut gateways));
    assert_eq!(*log.borrow(), ["transfer 1", "equip 1"]);
    assert_eq!(output.exit_code, 0, "{}", output.value);
    assert_eq!(output.value["kind"], "build_apply");
    assert_eq!(output.value["result"], "applied");
    assert_eq!(output.value["local_state"], "updated");
    assert_eq!(output.value["dispatched"], 2);
    let moved = item(&f, "1");
    assert_eq!(
        (moved.location, moved.equipped),
        (ItemLocation::Character(CharacterId::new("1")), true)
    );
    assert!(!item(&f, "3").equipped);
    let text = output.render_text();
    assert!(
        text.starts_with("Applied Raid on character 1: applied (2 succeeded"),
        "{text}"
    );
}

#[test]
fn failed_move_skips_the_equip_and_exits_41() {
    let mut f = fixture();
    let (mut gateways, log) = gateways(Script {
        transfer: [Ok(TransferOutcome::Rejected(
            TransferRejection::DestinationFull,
        ))]
        .into(),
        ..Default::default()
    });
    let output = run(&mut f, false, Some(&mut gateways));
    assert_eq!(*log.borrow(), ["transfer 1"]);
    assert_eq!(output.exit_code, 41);
    assert_eq!(output.value["result"], "failed");
    assert_eq!(
        output.value["steps"][0]["outcomes"][0]["reason"],
        "destination_full"
    );
    assert_eq!(output.value["steps"][1]["outcomes"][0]["status"], "skipped");
    assert_eq!(output.value["local_state"], "refresh_pending");
    assert_eq!(item(&f, "1").location, ItemLocation::Vault);
}

#[test]
fn throttle_fail_fast_exits_20_with_one_attempt_and_no_local_change() {
    let mut f = fixture();
    let (mut gateways, log) = gateways(Script {
        transfer: [Err(AppError::new(ErrorCategory::ProviderRateLimit))].into(),
        ..Default::default()
    });
    let output = run(&mut f, false, Some(&mut gateways));
    assert_eq!(log.borrow().len(), 1);
    assert_eq!(output.exit_code, 20);
    assert_eq!(output.value["result"], "refused");
    assert_eq!(output.value["local_state"], "unchanged");
    assert_eq!(item(&f, "1").location, ItemLocation::Vault);
}

#[test]
fn execution_without_gateways_is_unsupported_not_silent() {
    let mut f = fixture();
    let output = run(&mut f, false, None);
    assert_eq!(output.exit_code, 40);
    assert_eq!(output.value["error"]["code"], "unsupported_operation");
}

#[test]
fn local_state_persistence_failure_exits_42_after_bungie_acknowledged() {
    // Bungie acknowledges both actions while another process replaces the stored
    // profile without the moved item, so the acknowledged effects cannot be written.
    let path =
        std::env::temp_dir().join(format!("ghostlink-build-apply-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut f = fixture_in(SqliteStore::open(&path).unwrap());
    let other = path.clone();
    let (mut gateways, log) = gateways(Script {
        transfer: [Ok(TransferOutcome::Transferred)].into(),
        equip: [Ok(EquipOutcome::Equipped)].into(),
        during_transfer: Some(Box::new(move || {
            SqliteStore::open(&other)
                .unwrap()
                .replace_current_profile(profile_input(false), None, Some(vec![]))
                .unwrap();
        })),
    });
    let output = run(&mut f, false, Some(&mut gateways));
    let _ = std::fs::remove_file(&path);
    assert_eq!(*log.borrow(), ["transfer 1", "equip 1"]);
    assert_eq!(output.exit_code, 42, "{}", output.value);
    assert_eq!(output.value["result"], "applied");
    assert_eq!(output.value["dispatched"], 2);
    assert_eq!(output.value["counts"]["succeeded"], 2);
    assert_eq!(output.value["local_state"], "failed");
    assert_eq!(
        output.value["reason"],
        "local state could not be updated after the action"
    );
    assert!(
        output
            .render_text()
            .ends_with("local state could not be updated after the action\nLocal state: failed")
    );
}

#[test]
fn build_help_describes_apply_with_sockets_mods_and_fashion() {
    let help = destiny_cli::parse_process(["ghostlink", "build", "--help"]).unwrap_err();
    assert_eq!(help.exit_code, 0);
    assert!(help.stderr.is_empty());
    let apply = help
        .stdout
        .lines()
        .find(|l| l.trim_start().starts_with("apply "))
        .expect("apply line");
    assert!(!apply.contains("not applied yet"), "{apply}");
    assert!(
        apply.contains("subclass, artifact, mod and fashion plugs"),
        "{apply}"
    );
}
