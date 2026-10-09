use destiny_adapters::sqlite::SqliteStore;
use destiny_app::test_support::{
    DeterministicPlanIds, FakeClock, QueuedEquipGateway, QueuedProfileGateway,
    QueuedTransferGateway,
};
use destiny_app::*;
use destiny_cli::command::EquipArgs;
use destiny_mcp::service::MutationToolService;
use destiny_mcp::{ComposedMutationService, schema::EquipItemArgs};

const NOW: Timestamp = Timestamp::new(100);
const POLICY: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };
fn target() -> ItemLocation {
    ItemLocation::Character(CharacterId::new("warlock"))
}
fn snapshot(equipped: bool, on_target: bool, ambiguous: bool) -> InventorySnapshotInput {
    let character = Character {
        id: CharacterId::new("warlock"),
        name: None,
        class: Some("Warlock".into()),
        race: None,
        gender: None,
        light: None,
        date_last_played: None,
    };
    let item = |id, hash, location, equipped| ItemInstance {
        instance_id: ItemInstanceId::new(id),
        item_hash: ItemHash::new(hash),
        location,
        quantity: 1,
        equipped,
        locked: false,
        power: None,
        stats: vec![],
        plugs: vec![],
        perks: vec![],
        available_plugs: vec![],
    };
    let mut items = vec![
        item(
            "123",
            100,
            if on_target {
                target()
            } else {
                ItemLocation::Vault
            },
            equipped,
        ),
        item("124", 101, target(), true),
        item("125", 102, target(), true),
    ];
    if ambiguous {
        items.push(item("126", 100, ItemLocation::Vault, false));
    }
    InventorySnapshotInput {
        profile_currencies_coverage: Default::default(),
        stacks: Vec::new(),
        membership_id: MembershipId::new("789"),
        membership_type: MembershipType::new(3),
        checked_at: NOW,
        source: SourceMarker::Profile,
        definition_version: DefinitionVersion::new("v1"),
        characters: vec![character],
        items,
        plug_set_entries: vec![],
    }
}
fn definitions() -> Vec<ItemDefinition> {
    let definition = |hash, name: &str, bucket| ItemDefinition {
        does_postmaster_pull_have_side_effects: None,
        is_instance_item: None,
        max_stack_size: None,
        stack_unique_label: None,
        hash: ItemHash::new(hash),
        bucket_hash: Some(BucketHash::new(bucket)),
        name: name.into(),
        item_type: None,
        tier: None,
        element: None,
        slot: None,
        class_type: None,
        plug_category: None,
        tags: vec![],
        version: DefinitionVersion::new("v1"),
    };
    let mut equipment = definition(100, "Ace of Spades", 1498876634);
    equipment.slot = Some(ItemSlot::Kinetic);
    equipment.tier = Some("Exotic".into());
    equipment.class_type = Some("any".parse().unwrap());
    vec![
        equipment,
        definition(101, "Ghost", 4023194814),
        definition(102, "Subclass", 3284755031),
    ]
}
type Mutations = MutationService<QueuedProfileGateway, SqliteStore, DeterministicPlanIds>;
fn service(scenario: &str) -> (Mutations, QueuedTransferGateway, QueuedEquipGateway) {
    let (sync, transfer, equip) = sync_service(scenario);
    (
        MutationService::new(sync, DeterministicPlanIds::starting_at(1)),
        transfer,
        equip,
    )
}
fn sync_service(
    scenario: &str,
) -> (
    ProfileSyncService<QueuedProfileGateway, SqliteStore>,
    QueuedTransferGateway,
    QueuedEquipGateway,
) {
    let noop = scenario == "equip-noop";
    let ambiguous = scenario == "equip-ambiguous";
    let initial = snapshot(noop, noop, ambiguous);
    let mut store = SqliteStore::open_in_memory().unwrap();
    store
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions: definitions(),
            },
            checked_at: NOW,
            source: SourceMarker::Manifest,
        })
        .unwrap();
    store.replace_current(initial.clone()).unwrap();
    let mut profile = QueuedProfileGateway::new();
    let membership =
        || DestinyMembership::new(MembershipType::new(3), MembershipId::new("789")).unwrap();
    profile.queue_membership(Ok(membership()));
    profile.queue_inventory(Ok(initial));
    profile.queue_membership(Ok(membership()));
    profile.queue_inventory(
        if matches!(
            scenario,
            "equip-refresh-failure" | "equip-auth-unavailable-refresh"
        ) {
            Err(AppError::new(ErrorCategory::ProviderUnavailable))
        } else if scenario == "equip-auth-refresh-failure" {
            Err(AppError::new(ErrorCategory::ProviderAuthentication))
        } else if scenario == "equip-provider-stale" {
            Ok(snapshot(false, false, ambiguous))
        } else {
            Ok(snapshot(
                !matches!(
                    scenario,
                    "equip-partial"
                        | "equip-uncertain-no-effect"
                        | "equip-contradiction"
                        | "equip-auth"
                ),
                true,
                ambiguous,
            ))
        },
    );
    let mut transfer = QueuedTransferGateway::new();
    transfer.queue(Ok(TransferOutcome::Transferred));
    let mut equip = QueuedEquipGateway::new();
    equip.queue(Ok(match scenario {
        "equip-auth"
        | "equip-auth-refresh-failure"
        | "equip-auth-concurrent"
        | "equip-auth-unavailable-refresh" => EquipOutcome::AuthenticationRefused,
        "equip-partial" => EquipOutcome::Rejected(EquipRejection::NotInSocialSpace),
        "equip-uncertain" | "equip-uncertain-no-effect" => EquipOutcome::Uncertain,
        _ => EquipOutcome::Equipped,
    }));
    (
        ProfileSyncService::new_with_definitions(profile, store),
        transfer,
        equip,
    )
}
pub fn cli_json(scenario: &str, item: &str, character: &str) -> serde_json::Value {
    let (mut mutations, mut transfer, mut equip) = service(scenario);
    destiny_cli::equip::equip_item(
        &EquipArgs {
            item: item.into(),
            character: character.into(),
        },
        &mut mutations,
        &mut transfer,
        &mut equip,
        &FakeClock::fixed(NOW),
        POLICY,
    )
    .map(|output| {
        if matches!(scenario, "equip-provider-stale" | "equip-contradiction") {
            assert_eq!(output.exit_code(), 0);
        }
        if scenario == "equip-auth-concurrent" {
            assert_ne!(output.exit_code(), 0);
            assert!(output.render_text().contains("authentication"));
        }
        output.json()
    })
    .unwrap()
}
pub fn mcp_json(scenario: &str, item: &str, character: &str) -> serde_json::Value {
    let (mutations, transfer, equip) = service(scenario);
    let output = ComposedMutationService::new(
        mutations,
        transfer,
        FakeClock::fixed(NOW),
        POLICY,
        PlanLifetime::MAX,
    )
    .with_equip(equip)
    .equip_item(&EquipItemArgs {
        item: item.into(),
        character: character.into(),
    })
    .unwrap();
    output.json()
}

/// Exercise the production server boundary with real planning, SQLite publication,
/// ordered execution and local acknowledged effects. Channels pause immediately
/// before the first provider effect (after ticket validation), never sleeps.
#[allow(
    dead_code,
    reason = "shared helper module is also compiled by stdio parity tests"
)]
pub fn serialized_publication() {
    publication_order(true);
}

/// The operation mutex is not FIFO: a following equip may precede the read.
#[allow(
    dead_code,
    reason = "shared helper module is also compiled by stdio parity tests"
)]
pub fn equip_before_publication() {
    publication_order(false);
}

fn publication_order(read_first: bool) {
    use serde_json::json;
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;
    const BOUND: Duration = Duration::from_secs(5);
    #[derive(Clone)]
    struct Publication {
        events: Arc<Mutex<Vec<&'static str>>>,
        committed: mpsc::Sender<()>,
    }
    #[derive(Clone)]
    struct SharedStore(Arc<Mutex<SqliteStore>>, Option<Publication>);
    impl InventoryRepository for SharedStore {
        fn apply_acknowledged_effects(
            &mut self,
            membership: &DestinyMembership,
            effects: &[destiny_app::InventoryEffect],
        ) -> Result<InventorySnapshot, AppError> {
            self.0
                .lock()
                .unwrap()
                .apply_acknowledged_effects(membership, effects)
        }
        fn freshness_invalidated(&self) -> Result<bool, AppError> {
            self.0.lock().unwrap().freshness_invalidated()
        }
        fn invalidate_freshness(&mut self) -> Result<(), AppError> {
            self.0.lock().unwrap().invalidate_freshness()
        }
        fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
            self.0.lock().unwrap().load_current()
        }
        fn replace_current(
            &mut self,
            input: InventorySnapshotInput,
        ) -> Result<InventorySnapshot, AppError> {
            let snapshot = self.0.lock().unwrap().replace_current(input)?;
            if let Some(publication) = &self.1 {
                publication.events.lock().unwrap().push("read_publish");
                publication.committed.send(()).unwrap();
            }
            Ok(snapshot)
        }
    }
    impl SearchRepository for SharedStore {
        fn search_definitions(
            &self,
            query: &DefinitionSearchQuery,
        ) -> Result<Vec<ItemDefinition>, AppError> {
            self.0.lock().unwrap().search_definitions(query)
        }
        fn search_instances(
            &self,
            query: &InstanceSearchQuery,
        ) -> Result<Vec<ItemInstance>, AppError> {
            self.0.lock().unwrap().search_instances(query)
        }
        fn search_instance_query_page(
            &self,
            query: &InstanceSearchQuery,
            execution: Option<&destiny_app::search_query::ResolvedQuery>,
        ) -> Result<(Vec<ItemInstance>, bool), AppError> {
            if execution.is_none() {
                return self.search_instance_page(query);
            }
            self.0
                .lock()
                .unwrap()
                .search_instance_query_page(query, execution)
        }
        fn lookup_instance(
            &self,
            id: &ItemInstanceId,
        ) -> Result<Resolution<ItemInstance>, AppError> {
            self.0.lock().unwrap().lookup_instance(id)
        }
    }
    impl DefinitionRepository for SharedStore {
        fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
            self.0.lock().unwrap().current_definition_version()
        }
        fn lookup_definition(
            &self,
            hash: ItemHash,
        ) -> Result<Resolution<ItemDefinition>, AppError> {
            self.0.lock().unwrap().lookup_definition(hash)
        }
        fn current_metadata(&self) -> Result<Option<SyncMetadata>, AppError> {
            self.0.lock().unwrap().current_metadata()
        }
        fn replace_definitions(
            &mut self,
            input: DefinitionReplacementInput,
        ) -> Result<SyncMetadata, AppError> {
            self.0.lock().unwrap().replace_definitions(input)
        }
    }
    impl destiny_app::WorldDefinitionRepository for SharedStore {
        fn lookup_item_description(&self, hash: ItemHash) -> Result<Option<String>, AppError> {
            self.0.lock().unwrap().lookup_item_description(hash)
        }
        fn lookup_milestone(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_app::MilestoneDefinition>, AppError> {
            self.0.lock().unwrap().lookup_milestone(hash)
        }
        fn lookup_activity(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_app::ActivityDefinition>, AppError> {
            self.0.lock().unwrap().lookup_activity(hash)
        }
        fn lookup_activity_modifier(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_app::ActivityModifierDefinition>, AppError> {
            self.0.lock().unwrap().lookup_activity_modifier(hash)
        }
        fn lookup_objective(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_app::ObjectiveDefinition>, AppError> {
            self.0.lock().unwrap().lookup_objective(hash)
        }
        fn lookup_activity_type(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_app::ActivityTypeDefinition>, AppError> {
            self.0.lock().unwrap().lookup_activity_type(hash)
        }
        fn lookup_loadout_name(&self, hash: u32) -> Result<Option<String>, AppError> {
            self.0.lock().unwrap().lookup_loadout_name(hash)
        }
        fn world_definition_counts(&self) -> Result<[usize; 5], AppError> {
            self.0.lock().unwrap().world_definition_counts()
        }
        fn publish_manifest(
            &mut self,
            items: DefinitionReplacementInput,
            world: destiny_app::WorldDefinitionsAcquisition,
        ) -> Result<SyncMetadata, AppError> {
            self.0.lock().unwrap().publish_manifest(items, world)
        }
    }
    impl StatDefinitionRepository for SharedStore {
        fn lookup_stat(&self, hash: u32) -> Result<Option<StatDefinition>, AppError> {
            self.0.lock().unwrap().lookup_stat(hash)
        }
        fn stat_hashes_by_name(&self, name: &str) -> Result<Vec<u32>, AppError> {
            self.0.lock().unwrap().stat_hashes_by_name(name)
        }
        fn stat_definition_count(&self) -> Result<usize, AppError> {
            self.0.lock().unwrap().stat_definition_count()
        }
    }
    struct BeforeDispatch {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
        events: Arc<Mutex<Vec<&'static str>>>,
    }
    impl TransferGateway for BeforeDispatch {
        fn transfer(&mut self, _: &TransferRequest) -> Result<TransferOutcome, AppError> {
            self.entered.send(()).unwrap();
            self.release
                .recv_timeout(BOUND)
                .expect("dispatch released without deadlock");
            self.events.lock().unwrap().push("dispatch");
            Ok(TransferOutcome::Transferred)
        }
    }
    struct TrackedProfile(Arc<Mutex<QueuedProfileGateway>>);
    impl ProfileGateway for TrackedProfile {
        fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
            self.0.lock().unwrap().resolve_membership()
        }
        fn load_inventory(
            &mut self,
            membership: &DestinyMembership,
        ) -> Result<InventorySnapshotInput, AppError> {
            self.0.lock().unwrap().load_inventory(membership)
        }
    }
    struct PublishingProfile;
    impl ProfileGateway for PublishingProfile {
        fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
            DestinyMembership::new(MembershipType::new(3), MembershipId::new("789"))
        }
        fn load_inventory(
            &mut self,
            _: &DestinyMembership,
        ) -> Result<InventorySnapshotInput, AppError> {
            let mut newer = snapshot(true, true, false);
            newer.items[0].locked = true;
            Ok(newer)
        }
    }
    for equip in [false, true] {
        let (sync, _, equip_gateway) = sync_service("equip-success");
        let (_, store) = sync.into_parts();
        let mut profile = QueuedProfileGateway::new();
        profile.queue_membership(DestinyMembership::new(
            MembershipType::new(3),
            MembershipId::new("789"),
        ));
        profile.queue_inventory(Ok(snapshot(false, false, false)));
        profile.queue_membership(DestinyMembership::new(
            MembershipType::new(3),
            MembershipId::new("789"),
        ));
        let mut noop_snapshot = snapshot(true, true, false);
        noop_snapshot.items[0].locked = true;
        profile.queue_inventory(Ok(noop_snapshot));
        let profile = Arc::new(Mutex::new(profile));
        let store = SharedStore(Arc::new(Mutex::new(store)), None);
        let events = Arc::new(Mutex::new(Vec::new()));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (published_tx, published_rx) = mpsc::channel();
        let (attempt_tx, attempt_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let transfer = BeforeDispatch {
            entered: entered_tx,
            release: release_rx,
            events: events.clone(),
        };
        let mutations = MutationService::new(
            ProfileSyncService::new_with_definitions(
                TrackedProfile(profile.clone()),
                store.clone(),
            ),
            DeterministicPlanIds::starting_at(1),
        );
        let mutation = ComposedMutationService::new(
            mutations,
            transfer,
            FakeClock::fixed(NOW),
            POLICY,
            PlanLifetime::MAX,
        )
        .with_equip(equip_gateway);
        let mut read_store = store.clone();
        read_store.1 = Some(Publication {
            events: events.clone(),
            committed: published_tx,
        });
        let exclusive = Arc::new(Mutex::new(()));
        let read = destiny_mcp::QueryReadService::new(
            QueryService::new(
                ProfileSyncService::new_with_definitions(PublishingProfile, read_store)
                    .with_sync_gate(exclusive.clone()),
            ),
            FakeClock::fixed(Timestamp::new(1000)),
            POLICY,
        );
        let server = Arc::new(destiny_mcp::ReadToolServer::with_read_pool(
            vec![Box::new(read)],
            Box::new(mutation),
            exclusive,
        ));
        let (mutation_finished_tx, mutation_finished_rx) = mpsc::channel();
        let mutation_server = server.clone();
        let mutation_done = done_tx.clone();
        let mutation_thread = std::thread::spawn(move || {
            let (name, arguments) = if equip {
                ("equip_item", json!({"item":"123", "character":"warlock"}))
            } else {
                (
                    "move_item",
                    json!({"instance_id":"123", "character_id":"warlock"}),
                )
            };
            let output = mutation_server
                .dispatch(name, arguments.as_object().unwrap().clone())
                .unwrap();
            mutation_done.send(()).unwrap();
            mutation_finished_tx.send(()).unwrap();
            output
        });
        entered_rx
            .recv_timeout(BOUND)
            .expect("ticket validated and first dispatch reached");
        let (noop_finished_tx, noop_finished_rx) = mpsc::channel();
        let (read_finished_tx, read_finished_rx) = mpsc::channel();
        let read_server = server.clone();
        let read_thread = std::thread::spawn(move || {
            attempt_tx.send(()).unwrap();
            if !read_first {
                noop_finished_rx.recv_timeout(BOUND).unwrap();
            }
            let output = read_server
                .dispatch("list_characters", Default::default())
                .unwrap();
            done_tx.send(()).unwrap();
            if read_first {
                read_finished_tx.send(()).unwrap();
            }
            output
        });
        attempt_rx.recv_timeout(BOUND).unwrap();
        // Thread-start signals do not establish mutex acquisition order. Sequence
        // the read and following equip explicitly, outside the operation guard.
        // Ordinary sync in the following equip must not reenter that guard.
        let (noop_tx, noop_rx) = mpsc::channel();
        let (noop_attempt_tx, noop_attempt_rx) = mpsc::channel();
        let noop_server = server.clone();
        let noop_store = store.clone();
        let noop_thread = std::thread::spawn(move || {
            noop_attempt_tx.send(()).unwrap();
            mutation_finished_rx.recv_timeout(BOUND).unwrap();
            if read_first {
                read_finished_rx.recv_timeout(BOUND).unwrap();
            }
            let before = noop_store.load_current().unwrap().unwrap();
            let item = before
                .items
                .iter()
                .find(|item| item.instance_id.as_str() == "123")
                .unwrap();
            assert_eq!(item.location, target());
            assert_eq!(item.equipped, read_first || equip);
            assert_eq!(item.locked, read_first);
            assert_eq!(
                before.metadata.revision.value(),
                if read_first { 3 } else { 2 }
            );
            assert_eq!(before.metadata.checked_at, NOW);
            assert!(!noop_store.freshness_invalidated().unwrap());
            assert_eq!(noop_store.profile_minted_at().unwrap(), None);
            // At mutation NOW=100, checked_at=100 is fresh. Neither queued
            // profile entry is consumed; the equip plans from this local state.
            let noop = noop_server
                .dispatch(
                    "equip_item",
                    json!({"item":"123", "character":"warlock"})
                        .as_object()
                        .unwrap()
                        .clone(),
                )
                .unwrap();
            let after = noop_store.load_current().unwrap().unwrap();
            assert_eq!(
                after.metadata.revision.value(),
                before.metadata.revision.value() + u64::from(!read_first && !equip)
            );
            assert!(
                after
                    .items
                    .iter()
                    .find(|item| item.instance_id.as_str() == "123")
                    .unwrap()
                    .equipped
            );
            assert_eq!(after.metadata.checked_at, NOW);
            assert!(!noop_store.freshness_invalidated().unwrap());
            noop_tx.send(noop).unwrap();
            if !read_first {
                noop_finished_tx.send(()).unwrap();
            }
        });
        noop_attempt_rx.recv_timeout(BOUND).unwrap();
        // Bounded timeout detects the old independent service locks. Release
        // before asserting so even RED leaves no blocked worker behind.
        let overtook = published_rx
            .recv_timeout(Duration::from_millis(100))
            .is_ok();
        release_tx.send(()).unwrap();
        done_rx
            .recv_timeout(BOUND)
            .expect("mutation/post-refresh completed");
        done_rx.recv_timeout(BOUND).expect("queued read completed");
        let mutation_output = mutation_thread.join().unwrap();
        assert!(!mutation_output.is_error.unwrap_or(false));
        assert!(!read_thread.join().unwrap().is_error.unwrap_or(false));
        assert!(!overtook, "read publication overtook authorized dispatch");
        assert_eq!(*events.lock().unwrap(), ["dispatch", "read_publish"]);
        let current = store.load_current().unwrap().unwrap();
        assert!(
            current
                .items
                .iter()
                .find(|i| i.instance_id.as_str() == "123")
                .unwrap()
                .locked
        );
        let noop = noop_rx
            .recv_timeout(BOUND)
            .expect("no-op completed without reentrant deadlock");
        noop_thread.join().unwrap();
        assert_eq!(
            noop.structured_content.unwrap()["status"],
            if read_first || equip {
                "verified_noop"
            } else {
                "applied"
            }
        );
        assert_eq!(
            current.metadata.revision.value(),
            if !read_first && !equip { 4 } else { 3 }
        );
        let profile = profile.lock().unwrap();
        assert_eq!(profile.membership_calls(), 0);
        assert_eq!(profile.inventory_calls(), 0);
    }
}

pub fn reject_before_io() {
    // The seeded store models absence/staleness, but every service-side read
    // must remain unreachable. Membership resolution is the authentication seam.
    struct ForbiddenRepository {
        _store: SqliteStore,
    }
    impl InventoryRepository for ForbiddenRepository {
        fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
            panic!("repository read before argument rejection")
        }
        fn replace_current(
            &mut self,
            _: InventorySnapshotInput,
        ) -> Result<InventorySnapshot, AppError> {
            panic!("repository write before argument rejection")
        }
    }
    impl SearchRepository for ForbiddenRepository {
        fn search_definitions(
            &self,
            _: &DefinitionSearchQuery,
        ) -> Result<Vec<ItemDefinition>, AppError> {
            panic!("definition search before argument rejection")
        }
        fn search_instances(&self, _: &InstanceSearchQuery) -> Result<Vec<ItemInstance>, AppError> {
            panic!("instance search before argument rejection")
        }
        fn lookup_instance(
            &self,
            _: &ItemInstanceId,
        ) -> Result<Resolution<ItemInstance>, AppError> {
            panic!("instance lookup before argument rejection")
        }
    }
    impl DefinitionRepository for ForbiddenRepository {
        fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
            panic!("definition version before argument rejection")
        }
        fn lookup_definition(&self, _: ItemHash) -> Result<Resolution<ItemDefinition>, AppError> {
            panic!("definition lookup before argument rejection")
        }
        fn current_metadata(&self) -> Result<Option<SyncMetadata>, AppError> {
            panic!("definition metadata before argument rejection")
        }
        fn replace_definitions(
            &mut self,
            _: DefinitionReplacementInput,
        ) -> Result<SyncMetadata, AppError> {
            panic!("definition write before argument rejection")
        }
    }
    struct ForbiddenProfile;
    impl ProfileGateway for ForbiddenProfile {
        fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
            panic!("authenticated membership/credential access before argument rejection")
        }
        fn load_inventory(
            &mut self,
            _: &DestinyMembership,
        ) -> Result<InventorySnapshotInput, AppError> {
            panic!("profile access before argument rejection")
        }
    }
    struct ForbiddenClock;
    impl Clock for ForbiddenClock {
        fn now(&self) -> Result<Timestamp, AppError> {
            panic!("invalid arguments reached clock; all subsequent I/O must be unreachable");
        }
    }
    for stale in [false, true] {
        for item in ["x".repeat(257), "İ".repeat(129)] {
            let mut store = SqliteStore::open_in_memory().unwrap();
            if stale {
                let mut input = snapshot(false, false, false);
                input.checked_at = Timestamp::new(0);
                store.replace_current(input).unwrap();
            }
            let mutations = MutationService::new(
                ProfileSyncService::new(ForbiddenProfile, ForbiddenRepository { _store: store }),
                DeterministicPlanIds::starting_at(1),
            );
            let error = ComposedMutationService::new(
                mutations,
                QueuedTransferGateway::new(),
                ForbiddenClock,
                FreshnessPolicy { ttl_seconds: 1 },
                PlanLifetime::MAX,
            )
            .with_equip(QueuedEquipGateway::new())
            .equip_item(&EquipItemArgs {
                item,
                character: "warlock".into(),
            })
            .unwrap_err();
            assert_eq!(
                error,
                destiny_mcp::schema::McpError::new(ErrorCategory::ProviderValidation)
            );
        }
    }
}
