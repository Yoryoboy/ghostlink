//! `apply_build` through the MCP mutation seam with fakes only: the same executor and
//! shared output as `ghostlink build apply`, so the JSON and the failure class match.
use clap::Parser;
use destiny_adapters::{dim_loadout::DimBuildCodec, sqlite::SqliteStore};
use destiny_app::build_apply_exec::BuildGateways;
use destiny_app::builds::{BuildSelector, BuildService, LocalBuildRequest};
use destiny_app::domain::{ClassType, ItemSlot};
use destiny_app::*;
use destiny_mcp::apply_build::ApplyBuildMutationService;
use destiny_mcp::schema::{ApplyBuildArgs, apply_build_tool};
use destiny_mcp::service::{MoveItemRequest, MutationToolService};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

const KINETIC: i64 = 1498876634;
const FRESH: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };

fn args(wire: serde_json::Value) -> ApplyBuildArgs {
    serde_json::from_value(wire).unwrap()
}

#[test]
fn cli_and_mcp_build_apply_requests_are_identical() {
    for (cli, wire) in [
        (
            vec!["ghostlink", "build", "apply", "b1", "--character", "hunter"],
            serde_json::json!({"id":"b1", "character":"hunter"}),
        ),
        (
            vec!["ghostlink", "build", "apply", "--name", "Raid", "--dry-run"],
            serde_json::json!({"name":"Raid", "dry_run":true}),
        ),
    ] {
        let cli = destiny_cli::command::Cli::try_parse_from(cli).unwrap();
        let destiny_cli::command::Command::Build(build) = cli.command else {
            panic!()
        };
        let destiny_cli::command::BuildCommand::Apply(cli) = build.command else {
            panic!()
        };
        let mcp = args(wire);
        assert_eq!(cli.request().unwrap(), mcp.request().unwrap());
        assert_eq!(cli.dry_run, mcp.dry_run.unwrap_or(false));
    }
    for wire in [
        serde_json::json!({}),
        serde_json::json!({"id":"b1", "name":"Raid"}),
        serde_json::json!({"id":"b1", "character":"\n"}),
    ] {
        assert!(args(wire).request().is_err());
    }
    assert!(
        serde_json::from_value::<ApplyBuildArgs>(serde_json::json!({"id":"b1", "force":true}))
            .is_err()
    );
}

#[test]
fn descriptor_is_closed_small_and_has_no_root_combinator() {
    let tool = apply_build_tool();
    assert!(tool.description.as_ref().unwrap().len() <= 500);
    assert_eq!(tool.input_schema["additionalProperties"], false);
    for key in ["oneOf", "anyOf", "allOf"] {
        assert!(tool.input_schema.get(key).is_none(), "root {key}");
    }
    assert_eq!(
        tool.input_schema["properties"]["dry_run"]["type"],
        serde_json::json!(["boolean", "null"])
    );
    assert_eq!(tool.output_schema.as_ref().unwrap()["type"], "object");
    let annotations = tool.annotations.unwrap();
    assert_eq!(annotations.read_only_hint, Some(false));
    assert_eq!(annotations.destructive_hint, Some(false));
}

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
fn profile_input(with_item_1: bool) -> InventorySnapshotInput {
    InventorySnapshotInput {
        membership_id: MembershipId::new("99"),
        membership_type: MembershipType::new(3),
        definition_version: DefinitionVersion::new("v1"),
        checked_at: Timestamp::new(100),
        source: SourceMarker::Profile,
        characters: vec![Character {
            id: CharacterId::new("1"),
            class: Some("Hunter".into()),
            name: None,
            race: None,
            gender: None,
            light: None,
            date_last_played: None,
        }],
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

#[derive(Default)]
struct Script {
    transfer: VecDeque<Result<TransferOutcome, AppError>>,
    equip: VecDeque<Result<EquipOutcome, AppError>>,
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
impl destiny_app::InsertPlugGateway for Recorder {
    fn insert_plug(
        &mut self,
        _: &destiny_app::InsertPlugRequest,
    ) -> Result<destiny_app::InsertPlugOutcome, AppError> {
        panic!("unexpected mutation: insert_plug")
    }
}
struct Inner;
impl MutationToolService for Inner {
    fn move_item(
        &mut self,
        _: &MoveItemRequest,
    ) -> Result<destiny_mcp::schema::MoveItemOutput, destiny_mcp::McpError> {
        unreachable!("the decorator owns apply_build")
    }
}
type Decorator = ApplyBuildMutationService<
    Inner,
    Profile,
    SqliteStore,
    destiny_app::test_support::InMemoryBuildRepository,
    DimBuildCodec,
    destiny_app::test_support::FakeClock,
    Recorder,
    Recorder,
    Recorder,
    Recorder,
    Recorder,
>;
fn decorator(
    repository: SqliteStore,
    script: Script,
) -> (Decorator, String, Rc<RefCell<Vec<String>>>) {
    let mut repository = repository;
    repository
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions: vec![definition(100), definition(101)],
            },
            checked_at: Timestamp::new(100),
            source: SourceMarker::Manifest,
        })
        .unwrap();
    repository
        .replace_current_profile(profile_input(true), None, Some(vec![]))
        .unwrap();
    let mut builds = BuildService::new(
        destiny_app::test_support::InMemoryBuildRepository::default(),
        DimBuildCodec,
    );
    let membership =
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("99")).unwrap();
    builds
        .execute_local(
            &membership,
            &LocalBuildRequest::Import(
                r#"{"id":"b1","name":"Raid","classType":1,"equipped":[{"id":"1","hash":100}],"unequipped":[]}"#
                    .into(),
            ),
            Timestamp::new(100),
        )
        .unwrap();
    let id = builds
        .show(&membership, &BuildSelector::Name("Raid".into()))
        .unwrap()
        .definition
        .id;
    let log = Rc::new(RefCell::new(vec![]));
    let script = Rc::new(RefCell::new(script));
    let recorder = || Recorder {
        log: log.clone(),
        script: script.clone(),
    };
    let service = ApplyBuildMutationService {
        inner: Inner,
        builds,
        sync: ProfileSyncService::new_with_definitions(Profile(profile_input(true)), repository),
        gateways: BuildGateways {
            transfer: recorder(),
            equip: recorder(),
            equip_items: recorder(),
            pull: recorder(),
            socket: recorder(),
        },
        clock: destiny_app::test_support::FakeClock::fixed(Timestamp::new(100)),
        policy: FRESH,
        last_attempt: None,
    };
    (service, id, log)
}
fn working() -> Script {
    Script {
        transfer: [Ok(TransferOutcome::Transferred)].into(),
        equip: [Ok(EquipOutcome::Equipped)].into(),
        ..Default::default()
    }
}

#[test]
fn dry_run_plans_without_any_dispatch() {
    let (mut service, id, log) =
        decorator(SqliteStore::open_in_memory().unwrap(), Script::default());
    let output = service
        .apply_build(&args(serde_json::json!({"id":id, "dry_run":true})))
        .unwrap();
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.value["kind"], "build_apply_plan");
    assert_eq!(output.value["dispatched"], 0);
    assert!(log.borrow().is_empty());
}

#[test]
fn apply_executes_and_returns_the_cli_json() {
    let (mut service, id, log) = decorator(SqliteStore::open_in_memory().unwrap(), working());
    let output = service
        .apply_build(&args(serde_json::json!({"id":id})))
        .unwrap();
    assert_eq!(*log.borrow(), ["transfer 1", "equip 1"]);
    assert_eq!(output.exit_code, 0, "{}", output.value);
    assert_eq!(output.value["kind"], "build_apply");
    assert_eq!(output.value["result"], "applied");
    assert_eq!(output.value["local_state"], "updated");
    assert_eq!(output.value["dispatched"], 2);
}

#[test]
fn unknown_build_is_a_typed_output_without_dispatch() {
    let (mut service, _, log) =
        decorator(SqliteStore::open_in_memory().unwrap(), Script::default());
    let output = service
        .apply_build(&args(serde_json::json!({"id":"missing"})))
        .unwrap();
    assert_ne!(output.exit_code, 0);
    assert_eq!(output.value["error"]["code"], "item_not_found");
    assert!(log.borrow().is_empty());
}

#[test]
fn invalid_selector_is_rejected_before_any_work() {
    let (mut service, _, log) =
        decorator(SqliteStore::open_in_memory().unwrap(), Script::default());
    assert!(service.apply_build(&args(serde_json::json!({}))).is_err());
    assert!(log.borrow().is_empty());
}

#[test]
fn local_state_persistence_failure_matches_the_cli_exit_42() {
    let path = std::env::temp_dir().join(format!("ghostlink-mcp-apply-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let other = path.clone();
    let (mut service, id, log) = decorator(
        SqliteStore::open(&path).unwrap(),
        Script {
            during_transfer: Some(Box::new(move || {
                SqliteStore::open(&other)
                    .unwrap()
                    .replace_current_profile(profile_input(false), None, Some(vec![]))
                    .unwrap();
            })),
            ..working()
        },
    );
    let output = service
        .apply_build(&args(serde_json::json!({"id":id})))
        .unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(*log.borrow(), ["transfer 1", "equip 1"]);
    assert_eq!(output.exit_code, 42, "{}", output.value);
    assert_eq!(output.value["result"], "applied");
    assert_eq!(output.value["local_state"], "failed");
    assert_eq!(
        output.value["reason"],
        "local state could not be updated after the action"
    );
}
