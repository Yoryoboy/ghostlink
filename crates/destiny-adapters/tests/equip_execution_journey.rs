//! Offline equip journey through real profile decoding, mutation policy, both
//! Bungie mutation adapters, and durable SQLite. Only HTTP and time are scripted.

use destiny_adapters::equip::{BungieEquipGateway, EQUIP_ENDPOINT, EquipTransport};
use destiny_adapters::profile_gateway::{
    BungieProfileGateway, MEMBERSHIP_ENDPOINT, ProfileApiKey, ProfileHttpRequest,
    ProfileHttpResponse, ProfileTransport, ProfileTransportError, profile_endpoint,
};
use destiny_adapters::sqlite::SqliteStore;
use destiny_adapters::transfer::{
    BungieTransferGateway, TRANSFER_ENDPOINT, TransferApiKey, TransferHttpRequest,
    TransferHttpResponse, TransferTransport, TransferTransportError,
};
use destiny_app::test_support::DeterministicPlanIds;
use destiny_app::{MoveExecutionOutcome, MutationService, ProfileSyncService};
use destiny_core::domain::{ClassType, ItemSlot};
use destiny_core::{
    AccessCredential, AccessCredentialMetadata, AppError, BucketHash, CharacterId, Clock,
    CredentialMaterial, DefinitionReplacementInput, DefinitionRepository, DefinitionVersion,
    DestinationSelector, DestinyMembership, EquipRejection, EquipRequest, FreshnessPolicy,
    InventoryRepository, ItemDefinition, ItemHash, ItemLocation, ItemSelector, ManifestDefinitions,
    MembershipId, MembershipType, MoveIntentAuthorization, PlanLifetime, ProfileSyncReason,
    SourceMarker, StaticCredentialSource, StepOutcome, Timestamp,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::io::Cursor;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

const TARGET: &str = "900000000000000001";
const OTHER: &str = "900000000000000002";
const ITEM: &str = "700000000000000001";
const MEMBER: &str = "4611686018484639825";
const VERSION: &str = "equip-journey-v1";
const NOW: Timestamp = Timestamp::new(1_724_000_000);
const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };
const KEY: &str = "sentinel-equip-journey-api-key";
const TOKEN: &str = "sentinel-equip-journey-bearer-token";
const SUCCESS: &str = r#"{"ErrorCode":1,"ErrorStatus":"Success","Response":{}}"#;

#[derive(Clone, Copy)]
enum Reply {
    Applied,
    Auth(u16),
    AuthConcurrent,
    AuthUnavailableRefresh,
    NotDispatched,
    Refused(u32, &'static str),
    Uncertain { lands: bool },
}

struct World {
    profile: Value,
    reply: Reply,
    mutations: Vec<(String, Value)>,
    profiles: Vec<String>,
    debug: Vec<String>,
    transfer_auth: Option<u16>,
    refresh_auth: bool,
}

impl World {
    fn new(reply: Reply) -> Self {
        let mut profile: Value =
            serde_json::from_slice(include_bytes!("fixtures/profile-complete.json")).unwrap();
        // A complete target equipment set: weapons, armor, and the non-equipment
        // neighbors that previously caused every exotic candidate to be refused.
        let buckets = [
            2465295065_u32,
            953998645,
            3448274439,
            3551918588,
            14239492,
            20886954,
            1585787867,
            3284755031,
            4023194814,
            4274335291,
        ];
        let mut equipment = Vec::new();
        for (index, bucket) in buckets.into_iter().enumerate() {
            let id = format!("7100000000000000{index:02}");
            equipment.push(json!({"itemHash":4000 + index, "itemInstanceId":id,
                "quantity":1, "location":1, "bucketHash":bucket, "state":0}));
            profile["Response"]["itemComponents"]["instances"]["data"][&id] =
                json!({"isEquipped":true});
        }
        profile["Response"]["characterEquipment"]["data"][TARGET]["items"] = json!(equipment);
        Self {
            profile,
            reply,
            mutations: Vec::new(),
            profiles: Vec::new(),
            debug: Vec::new(),
            transfer_auth: None,
            refresh_auth: false,
        }
    }

    fn take_item(&mut self) -> Value {
        let response = &mut self.profile["Response"];
        for component in [
            "profileInventory",
            "characterInventories",
            "characterEquipment",
        ] {
            let groups: Vec<Option<&str>> = if component == "profileInventory" {
                vec![None]
            } else {
                vec![Some(TARGET), Some(OTHER)]
            };
            for character in groups {
                let data = &mut response[component]["data"];
                let items = match character {
                    None => &mut data["items"],
                    Some(id) => &mut data[id]["items"],
                }
                .as_array_mut()
                .unwrap();
                if let Some(index) = items.iter().position(|item| item["itemInstanceId"] == ITEM) {
                    return items.remove(index);
                }
            }
        }
        panic!("tracked instance missing");
    }

    fn place(&mut self, character: Option<&str>, equipped: bool) {
        let mut item = self.take_item();
        item["location"] = json!(if character.is_some() { 1 } else { 2 });
        let response = &mut self.profile["Response"];
        let items = match character {
            None => &mut response["profileInventory"]["data"]["items"],
            Some(id) => {
                let component = if equipped {
                    "characterEquipment"
                } else {
                    "characterInventories"
                };
                &mut response[component]["data"][id]["items"]
            }
        };
        items.as_array_mut().unwrap().push(item);
        response["itemComponents"]["instances"]["data"][ITEM]["isEquipped"] = json!(equipped);
    }
}

#[derive(Clone)]
struct Http(Rc<RefCell<World>>);

fn profile_response(value: Value) -> ProfileHttpResponse {
    let body = serde_json::to_vec(&value).unwrap();
    ProfileHttpResponse::new(
        200,
        Some(body.len() as u64),
        Some("application/json".into()),
        Box::new(Cursor::new(body)),
    )
}

impl ProfileTransport for Http {
    fn get(
        &mut self,
        request: &ProfileHttpRequest<'_>,
    ) -> Result<ProfileHttpResponse, ProfileTransportError> {
        assert!(request.api_key().is_some() && request.bearer().is_some());
        let mut world = self.0.borrow_mut();
        world.debug.push(format!("{request:?}"));
        world.profiles.push(request.url().into());
        if (world.refresh_auth || matches!(world.reply, Reply::AuthUnavailableRefresh))
            && !world.mutations.is_empty()
        {
            return Ok(ProfileHttpResponse::new(
                if world.refresh_auth { 401 } else { 503 },
                None,
                None,
                Box::new(Cursor::new(format!("{KEY} {TOKEN}"))),
            ));
        }
        if request.url() == MEMBERSHIP_ENDPOINT {
            Ok(profile_response(json!({"ErrorCode":1,"Response":{
            "primaryMembershipId":MEMBER,"destinyMemberships":[{
                "membershipType":3,"membershipId":MEMBER,"crossSaveOverride":3,
                "applicableMembershipTypes":[3],"isPublic":true
            }]}})))
        } else {
            Ok(profile_response(world.profile.clone()))
        }
    }
}

fn mutation_response(status: u16, body: String) -> TransferHttpResponse {
    TransferHttpResponse::new(
        status,
        Some(body.len() as u64),
        Some("application/json".into()),
        Box::new(Cursor::new(body.into_bytes())),
    )
}

impl TransferTransport for Http {
    fn post(
        &mut self,
        request: &TransferHttpRequest<'_>,
    ) -> Result<TransferHttpResponse, TransferTransportError> {
        assert_eq!(request.url(), TRANSFER_ENDPOINT);
        assert!(request.api_key().is_some() && request.bearer().is_some());
        let body: Value = serde_json::from_str(request.body()).unwrap();
        assert_eq!(body["itemId"], ITEM);
        assert_eq!(body["membershipType"], 3);
        let mut world = self.0.borrow_mut();
        world.debug.push(format!("{request:?}"));
        world.mutations.push((request.url().into(), body.clone()));
        if let Some(status) = world.transfer_auth {
            return Ok(mutation_response(status, format!("{KEY} {TOKEN}")));
        }
        world.place(
            if body["transferToVault"] == true {
                None
            } else {
                Some(TARGET)
            },
            false,
        );
        Ok(mutation_response(200, SUCCESS.into()))
    }
}

impl EquipTransport for Http {
    fn post(
        &mut self,
        request: &TransferHttpRequest<'_>,
    ) -> Result<TransferHttpResponse, TransferTransportError> {
        assert_eq!(
            request.url(),
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/EquipItem/"
        );
        assert_eq!(
            serde_json::from_str::<Value>(request.body()).unwrap(),
            json!({"itemId":ITEM,"characterId":TARGET,"membershipType":3})
        );
        assert!(request.api_key().is_some() && request.bearer().is_some());
        let mut world = self.0.borrow_mut();
        if matches!(world.reply, Reply::NotDispatched) {
            return Err(TransferTransportError::NotDispatched);
        }
        world.debug.push(format!("{request:?}"));
        world.mutations.push((
            request.url().into(),
            serde_json::from_str(request.body()).unwrap(),
        ));
        match world.reply {
            Reply::Auth(status) => Ok(mutation_response(status, format!("{KEY} {TOKEN}"))),
            Reply::AuthConcurrent => {
                // Independent game action satisfies the target despite the refused call.
                world.place(Some(TARGET), true);
                Ok(mutation_response(403, format!("{KEY} {TOKEN}")))
            }
            Reply::AuthUnavailableRefresh => Ok(mutation_response(403, format!("{KEY} {TOKEN}"))),
            Reply::NotDispatched => unreachable!(),
            Reply::Applied => {
                world.place(Some(TARGET), true);
                Ok(mutation_response(200, SUCCESS.into()))
            }
            Reply::Refused(code, status) => Ok(mutation_response(200,
                json!({"ErrorCode":code,"ErrorStatus":status,"Message":format!("{KEY} {TOKEN}"),"Response":{}}).to_string())),
            Reply::Uncertain { lands } => {
                if lands { world.place(Some(TARGET), true); }
                Ok(mutation_response(503, format!("{KEY} {TOKEN}")))
            }
        }
    }
}

struct TestClock;
impl Clock for TestClock {
    fn now(&self) -> Result<Timestamp, AppError> {
        Ok(NOW)
    }
}

fn credentials() -> StaticCredentialSource {
    StaticCredentialSource::new(AccessCredential {
        metadata: AccessCredentialMetadata {
            expires_at: Timestamp::new(4_000_000_000),
        },
        material: CredentialMaterial::from_bytes(TOKEN.as_bytes().to_vec()),
        refresh_grant: None,
    })
}

fn definitions() -> Vec<ItemDefinition> {
    let slots = [
        ItemSlot::Kinetic,
        ItemSlot::Energy,
        ItemSlot::Power,
        ItemSlot::Helmet,
        ItemSlot::Gauntlets,
        ItemSlot::Chest,
        ItemSlot::Legs,
        ItemSlot::ClassItem,
        ItemSlot::Other,
        ItemSlot::Other,
        ItemSlot::Other,
    ];
    let buckets = [
        1498876634_u32,
        2465295065,
        953998645,
        3448274439,
        3551918588,
        14239492,
        20886954,
        1585787867,
        3284755031,
        4023194814,
        4274335291,
    ];
    slots
        .into_iter()
        .zip(buckets)
        .enumerate()
        .map(|(index, (slot, bucket))| ItemDefinition {
            hash: ItemHash::new(if index == 0 {
                1001
            } else {
                3999 + index as i64
            }),
            bucket_hash: Some(BucketHash::new(i64::from(bucket))),
            name: if index == 0 {
                "The Last Word".into()
            } else {
                format!("Neighbor {index}")
            },
            item_type: None,
            tier: if index == 0 {
                Some("Exotic".into())
            } else if index < 8 {
                Some("Legendary".into())
            } else {
                None
            },
            element: None,
            slot: Some(slot),
            class_type: Some(ClassType::Any),
            plug_category: None,
            tags: Vec::new(),
            version: DefinitionVersion::new(VERSION),
        })
        .collect()
}

static NEXT_DB: AtomicU64 = AtomicU64::new(0);

struct Journey {
    world: Rc<RefCell<World>>,
    database: PathBuf,
}

impl Journey {
    fn new(reply: Reply) -> Self {
        Self {
            world: Rc::new(RefCell::new(World::new(reply))),
            database: std::env::temp_dir().join(format!(
                "destiny-equip-journey-{}-{}.db",
                std::process::id(),
                NEXT_DB.fetch_add(1, Ordering::Relaxed)
            )),
        }
    }

    fn run(&self, transfers: usize, no_op: bool, equipped: bool) -> MoveExecutionOutcome {
        let profile = BungieProfileGateway::new(
            Http(self.world.clone()),
            TestClock,
            credentials(),
            ProfileApiKey::new(KEY).unwrap(),
            DefinitionVersion::new(VERSION),
        )
        .unwrap();
        let mut transfer = BungieTransferGateway::new(
            Http(self.world.clone()),
            TestClock,
            credentials(),
            TransferApiKey::new(KEY).unwrap(),
        )
        .unwrap();
        let mut equip = BungieEquipGateway::new(
            Http(self.world.clone()),
            TestClock,
            credentials(),
            TransferApiKey::new(KEY).unwrap(),
        )
        .unwrap();
        let mut debug = format!("{profile:?} {transfer:?} {equip:?}");
        let store = SqliteStore::open(&self.database).unwrap();
        let mut service = MutationService::new(
            ProfileSyncService::new(profile, store),
            DeterministicPlanIds::starting_at(1),
        );
        service
            .repository_mut()
            .replace_definitions(DefinitionReplacementInput {
                definitions: ManifestDefinitions {
                    version: DefinitionVersion::new(VERSION),
                    definitions: definitions(),
                },
                checked_at: NOW,
                source: SourceMarker::Manifest,
            })
            .unwrap();
        service
            .sync_mut()
            .sync(ProfileSyncReason::Ordinary, TTL, NOW)
            .unwrap();
        let request = EquipRequest {
            membership: MembershipId::new(MEMBER),
            item: ItemSelector::semantic("The Last Word").unwrap(),
            destination: DestinationSelector::character_label("Warlock").unwrap(),
            intent: MoveIntentAuthorization::Explicit,
        };
        let plan = service
            .plan_equip(&request, PlanLifetime::new(120).unwrap(), TTL, NOW)
            .unwrap();
        let before = self.world.borrow().profiles.len();
        let result = service.execute_equip_verified(
            (&mut transfer, &mut equip),
            &request,
            &plan,
            TTL,
            NOW,
            NOW,
        );
        debug.push_str(&format!("{plan:?} {result:?}"));
        let outcome = result.unwrap();
        assert_eq!(
            self.world.borrow().profiles.len() - before,
            if no_op { 2 } else { 4 },
            "forced preflight plus mandatory post-action membership/profile refresh"
        );
        let snapshot = service.repository().load_current().unwrap().unwrap();
        let item = snapshot
            .items
            .iter()
            .find(|item| item.instance_id.as_str() == ITEM)
            .unwrap();
        assert_eq!(
            item.location,
            ItemLocation::Character(CharacterId::new(TARGET))
        );
        assert_eq!(item.equipped, equipped);
        assert_eq!(
            snapshot
                .items
                .iter()
                .filter(|item| item.equipped
                    && item.location == ItemLocation::Character(CharacterId::new(TARGET)))
                .count(),
            10 + usize::from(equipped)
        );
        drop(service);
        // Independently reopen the durable repository, not just the gateway world.
        assert_eq!(
            SqliteStore::open(&self.database)
                .unwrap()
                .load_current()
                .unwrap()
                .unwrap(),
            snapshot
        );
        let world = self.world.borrow();
        assert_eq!(world.mutations.len(), transfers + usize::from(!no_op));
        for (index, (url, body)) in world.mutations.iter().enumerate() {
            if index < transfers {
                assert_eq!(url, TRANSFER_ENDPOINT);
                assert_eq!(
                    *body,
                    json!({"itemReferenceHash":1001,"stackSize":1,"transferToVault":transfers == 2 && index == 0,
                    "itemId":ITEM,"characterId":if transfers == 2 && index == 0 { OTHER } else { TARGET },"membershipType":3})
                );
            } else {
                assert_eq!(url, EQUIP_ENDPOINT);
                assert_eq!(
                    *body,
                    json!({"itemId":ITEM,"characterId":TARGET,"membershipType":3})
                );
            }
        }
        let membership =
            DestinyMembership::new(MembershipType::new(3), MembershipId::new(MEMBER)).unwrap();
        for url in &world.profiles {
            assert!(url == MEMBERSHIP_ENDPOINT || url == &profile_endpoint(&membership));
        }
        debug.push_str(&format!(
            "{:?} {:?} {snapshot:?}",
            world.debug, world.mutations
        ));
        for secret in [KEY, TOKEN] {
            assert!(!debug.contains(secret));
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let path = PathBuf::from(format!("{}{suffix}", self.database.display()));
                if suffix.is_empty() || path.exists() {
                    let bytes =
                        std::fs::read(path).expect("database and present sidecars are readable");
                    assert!(
                        !bytes
                            .windows(secret.len())
                            .any(|part| part == secret.as_bytes())
                    );
                }
            }
        }
        outcome
    }
}

impl Drop for Journey {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.database.display()));
        }
    }
}

// In-memory RC003 fixture: real adapters and profile decoding, scripted HTTP only.
fn rc003_attempt(
    reply: Reply,
    on_target: bool,
    expired: bool,
    transfer_auth: Option<u16>,
    refresh_auth: bool,
) -> (
    Result<MoveExecutionOutcome, destiny_app::ExecutionFailure>,
    Rc<RefCell<World>>,
) {
    let mut world = World::new(reply);
    if on_target {
        world.place(Some(TARGET), false);
    }
    world.transfer_auth = transfer_auth;
    world.refresh_auth = refresh_auth;
    let world = Rc::new(RefCell::new(world));
    let profile = BungieProfileGateway::new(
        Http(world.clone()),
        TestClock,
        credentials(),
        ProfileApiKey::new(KEY).unwrap(),
        DefinitionVersion::new(VERSION),
    )
    .unwrap();
    let mut transfer = BungieTransferGateway::new(
        Http(world.clone()),
        TestClock,
        credentials(),
        TransferApiKey::new(KEY).unwrap(),
    )
    .unwrap();
    let equip_credentials = if expired {
        StaticCredentialSource::new(AccessCredential {
            metadata: AccessCredentialMetadata { expires_at: NOW },
            material: CredentialMaterial::from_bytes(TOKEN.as_bytes().to_vec()),
            refresh_grant: None,
        })
    } else {
        credentials()
    };
    let mut equip = BungieEquipGateway::new(
        Http(world.clone()),
        TestClock,
        equip_credentials,
        TransferApiKey::new(KEY).unwrap(),
    )
    .unwrap();
    let mut service = MutationService::new(
        ProfileSyncService::new(profile, SqliteStore::open_in_memory().unwrap()),
        DeterministicPlanIds::starting_at(1),
    );
    service
        .repository_mut()
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new(VERSION),
                definitions: definitions(),
            },
            checked_at: NOW,
            source: SourceMarker::Manifest,
        })
        .unwrap();
    service
        .sync_mut()
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .unwrap();
    let request = EquipRequest {
        membership: MembershipId::new(MEMBER),
        item: ItemSelector::semantic("The Last Word").unwrap(),
        destination: DestinationSelector::character_label("Warlock").unwrap(),
        intent: MoveIntentAuthorization::Explicit,
    };
    let plan = service
        .plan_equip(&request, PlanLifetime::new(120).unwrap(), TTL, NOW)
        .unwrap();
    world.borrow_mut().profiles.clear();
    let result =
        service.execute_equip_verified((&mut transfer, &mut equip), &request, &plan, TTL, NOW, NOW);
    for secret in [KEY, TOKEN] {
        assert!(!format!("{result:?}").contains(secret));
    }
    (result, world)
}

fn assert_rc003_equip_auth(status: u16, on_target: bool) {
    {
        let (result, world) = rc003_attempt(Reply::Auth(status), on_target, false, None, false);
        let outcome = result.expect("dispatched auth refusal must retain evidence and refresh");
        let MoveExecutionOutcome::IncompleteExecution(incomplete) = outcome else {
            panic!("auth refusal cannot claim applied");
        };
        assert_eq!(
            incomplete.evidence().attempted_count(),
            if on_target { 1 } else { 2 }
        );
        assert_eq!(
            incomplete.evidence().completed_count(),
            usize::from(!on_target)
        );
        assert!(
            incomplete
                .error()
                .unwrap()
                .to_string()
                .contains("authentication")
        );
        assert_eq!(world.borrow().profiles.len(), 4);
        assert_eq!(
            world.borrow().mutations.len(),
            if on_target { 1 } else { 2 }
        );
    }
}

#[test]
fn rc003_auth_concurrent_target_remains_non_success() {
    for on_target in [true, false] {
        let (result, world) = rc003_attempt(Reply::AuthConcurrent, on_target, false, None, false);
        let MoveExecutionOutcome::IncompleteExecution(incomplete) = result.unwrap() else {
            panic!("definite authentication refusal must not become success");
        };
        assert_eq!(
            incomplete.error(),
            Some(destiny_app::MutationError::Application(
                destiny_core::ErrorCategory::ProviderAuthentication
            ))
        );
        assert_eq!(
            incomplete.observed(),
            &ItemLocation::Character(CharacterId::new(TARGET))
        );
        assert_eq!(
            incomplete.evidence().completed_count(),
            usize::from(!on_target)
        );
        assert_eq!(
            incomplete.evidence().attempted_count(),
            if on_target { 1 } else { 2 }
        );
        assert_eq!(
            incomplete.stop_outcome(),
            Some(StepOutcome::AuthenticationRefused)
        );
        assert_eq!(world.borrow().profiles.len(), 4);
        assert_eq!(
            world.borrow().profile["Response"]["itemComponents"]["instances"]["data"][ITEM]["isEquipped"],
            true
        );
    }
}

#[test]
fn rc003_auth_unavailable_refresh_preserves_prior_progress() {
    let (result, world) = rc003_attempt(Reply::AuthUnavailableRefresh, false, false, None, false);
    let MoveExecutionOutcome::VerificationFailed(failure) = result.unwrap() else {
        panic!("refresh failure must remain visible");
    };
    assert_eq!(failure.evidence().attempted_count(), 2);
    assert_eq!(failure.evidence().completed_count(), 1);
    assert_eq!(
        failure.evidence().stop_outcome(),
        Some(StepOutcome::AuthenticationRefused)
    );
    assert_eq!(
        failure.error(),
        Some(destiny_app::MutationError::Application(
            destiny_core::ErrorCategory::ProviderUnavailable
        ))
    );
    assert_eq!(
        failure.reason(),
        destiny_app::VerificationFailureReason::RefreshFailed
    );
    assert!(failure.observed().is_none());
    assert_eq!(world.borrow().profiles.len(), 3);
}

#[test]
fn rc003_equip_only_401() {
    assert_rc003_equip_auth(401, true);
}
#[test]
fn rc003_equip_only_403() {
    assert_rc003_equip_auth(403, true);
}
#[test]
fn rc003_transfer_then_equip_401() {
    assert_rc003_equip_auth(401, false);
}
#[test]
fn rc003_transfer_then_equip_403() {
    assert_rc003_equip_auth(403, false);
}

fn assert_rc003_transfer_auth(status: u16) {
    {
        let (result, world) = rc003_attempt(Reply::Applied, false, false, Some(status), false);
        let MoveExecutionOutcome::IncompleteExecution(incomplete) =
            result.expect("transfer auth dispatched")
        else {
            panic!("not applied");
        };
        assert_eq!(incomplete.evidence().attempted_count(), 1);
        assert_eq!(incomplete.evidence().completed_count(), 0);
        assert_eq!(incomplete.evidence().unattempted_count(), 1);
        assert!(
            incomplete
                .error()
                .unwrap()
                .to_string()
                .contains("authentication")
        );
        assert_eq!(world.borrow().mutations.len(), 1);
        assert_eq!(world.borrow().profiles.len(), 4);
    }
}
#[test]
fn rc003_transfer_auth_401() {
    assert_rc003_transfer_auth(401);
}
#[test]
fn rc003_transfer_auth_403() {
    assert_rc003_transfer_auth(403);
}

#[test]
fn rc003_predispatch_stays_unattempted_but_prior_transfer_requires_refresh() {
    for reply in [Reply::Applied, Reply::NotDispatched] {
        for on_target in [true, false] {
            let (result, world) = rc003_attempt(
                reply,
                on_target,
                matches!(reply, Reply::Applied),
                None,
                false,
            );
            if on_target {
                let failure = result.expect_err("no dispatch");
                assert_eq!(failure.evidence().attempted_count(), 0);
                assert_eq!(world.borrow().profiles.len(), 2);
                assert!(world.borrow().mutations.is_empty());
            } else {
                let MoveExecutionOutcome::IncompleteExecution(incomplete) = result.unwrap() else {
                    panic!("prior transfer must survive");
                };
                assert_eq!(incomplete.evidence().attempted_count(), 1);
                assert_eq!(incomplete.evidence().completed_count(), 1);
                assert_eq!(incomplete.evidence().unattempted_count(), 1);
                assert_eq!(world.borrow().profiles.len(), 4);
                assert_eq!(world.borrow().mutations.len(), 1);
            }
        }
    }
}

#[test]
fn rc003_auth_failed_refresh_preserves_original_attempt_without_observation() {
    let (result, world) = rc003_attempt(Reply::Auth(403), true, false, None, true);
    let MoveExecutionOutcome::VerificationFailed(failure) = result.expect("auth dispatched") else {
        panic!("refresh failed");
    };
    assert_eq!(failure.evidence().attempted_count(), 1);
    assert_eq!(failure.evidence().completed_count(), 0);
    assert_eq!(
        failure.evidence().stop_outcome(),
        Some(StepOutcome::AuthenticationRefused)
    );
    assert_eq!(
        failure.reason(),
        destiny_app::VerificationFailureReason::RefreshFailed
    );
    assert_eq!(
        failure.error(),
        Some(destiny_app::MutationError::Application(
            destiny_core::ErrorCategory::ProviderAuthentication
        ))
    );
    assert!(failure.observed().is_none());
    assert_eq!(world.borrow().profiles.len(), 3);
}

#[test]
fn vault_transfer_then_equip_is_verified() {
    assert!(matches!(
        Journey::new(Reply::Applied).run(1, false, true),
        MoveExecutionOutcome::VerifiedSuccess(_)
    ));
}

#[test]
fn other_character_transfers_via_vault_then_equips() {
    let journey = Journey::new(Reply::Applied);
    journey.world.borrow_mut().place(Some(OTHER), false);
    assert!(matches!(
        journey.run(2, false, true),
        MoveExecutionOutcome::VerifiedSuccess(_)
    ));
}

#[test]
fn already_equipped_is_verified_without_mutation() {
    let journey = Journey::new(Reply::Applied);
    journey.world.borrow_mut().place(Some(TARGET), true);
    assert!(matches!(
        journey.run(0, true, true),
        MoveExecutionOutcome::VerifiedNoOp(_)
    ));
}

fn assert_refusal(code: u32, status: &'static str, rejection: EquipRejection) {
    let journey = Journey::new(Reply::Refused(code, status));
    let MoveExecutionOutcome::IncompleteExecution(incomplete) = journey.run(1, false, false) else {
        panic!("expected incomplete execution");
    };
    assert!(incomplete.is_partial());
    assert_eq!(incomplete.evidence().completed_count(), 1);
    assert_eq!(
        incomplete.stop_outcome(),
        Some(StepOutcome::EquipRejected(rejection))
    );
}

#[test]
fn exotic_refusal_preserves_completed_transfer_and_refreshes() {
    assert_refusal(
        1641,
        "DestinyItemUniqueEquipRestricted",
        EquipRejection::ExoticLimit,
    );
}

#[test]
fn social_space_refusal_is_typed_and_refreshes() {
    assert_refusal(
        1634,
        "DestinyCharacterNotInTower",
        EquipRejection::NotInSocialSpace,
    );
}

#[test]
fn uncertain_equip_is_verified_only_when_refresh_proves_equipped() {
    let journey = Journey::new(Reply::Uncertain { lands: true });
    let MoveExecutionOutcome::VerifiedSuccess(evidence) = journey.run(1, false, true) else {
        panic!("expected verified success");
    };
    assert_eq!(evidence.stop_outcome(), Some(StepOutcome::Uncertain));
}

#[test]
fn uncertain_equip_without_effect_is_incomplete_not_success() {
    let journey = Journey::new(Reply::Uncertain { lands: false });
    let MoveExecutionOutcome::IncompleteExecution(incomplete) = journey.run(1, false, false) else {
        panic!("expected incomplete execution");
    };
    assert_eq!(incomplete.stop_outcome(), Some(StepOutcome::Uncertain));
    assert_eq!(incomplete.evidence().completed_count(), 1);
}
