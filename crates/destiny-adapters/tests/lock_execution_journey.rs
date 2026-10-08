//! Offline lock seam journey: real profile decoding, lock adapter and SQLite.
//! HTTP and time are scripted. App planning/verification belongs to IAC-004.

use destiny_adapters::decode_profile;
use destiny_adapters::lock::{
    BungieLockGateway, LOCK_ENDPOINT, LockApiKey, LockHttpRequest, LockHttpResponse, LockTransport,
    LockTransportError,
};
use destiny_adapters::sqlite::SqliteStore;
use destiny_core::{
    AccessCredential, AccessCredentialMetadata, AppError, CharacterId, Clock, CredentialMaterial,
    DefinitionVersion, DestinyMembership, InventoryRepository, ItemInstanceId, LockGateway,
    LockOutcome, LockRejection, MembershipId, MembershipType, SetLockStateRequest,
    StaticCredentialSource, Timestamp,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::io::Cursor;
use std::rc::Rc;

const ITEM: &str = "700000000000000001";
const CHARACTER: &str = "900000000000000001";
const KEY: &str = "sentinel-lock-journey-key";
const TOKEN: &str = "sentinel-lock-journey-token";

#[derive(Clone, Copy)]
enum Reply {
    Applied,
    Refused,
    Auth,
    Throttled,
    Uncertain { lands: bool },
}

struct World {
    profile: Value,
    reply: Reply,
    calls: usize,
    debug: Vec<String>,
}
struct Http(Rc<RefCell<World>>);
impl LockTransport for Http {
    fn post(
        &mut self,
        request: &LockHttpRequest<'_>,
    ) -> Result<LockHttpResponse, LockTransportError> {
        assert_eq!(request.url(), LOCK_ENDPOINT);
        assert!(request.api_key().is_some() && request.bearer().is_some());
        let body: Value = serde_json::from_str(request.body()).unwrap();
        assert_eq!(body["itemId"], ITEM);
        assert_eq!(body["characterId"], CHARACTER);
        assert_eq!(body["membershipType"], 3);
        let mut world = self.0.borrow_mut();
        world.calls += 1;
        world.debug.push(format!("{request:?}"));
        if matches!(
            world.reply,
            Reply::Applied | Reply::Uncertain { lands: true }
        ) {
            let items = world.profile["Response"]["profileInventory"]["data"]["items"]
                .as_array_mut()
                .unwrap();
            let item = items
                .iter_mut()
                .find(|item| item["itemInstanceId"] == ITEM)
                .unwrap();
            item["state"] = json!(if body["state"] == true { 1 } else { 0 });
        }
        let (status, response) = match world.reply {
            Reply::Applied => (200, json!({"ErrorCode":1,"Response":0})),
            Reply::Refused => (
                200,
                json!({"ErrorCode":1623,"ErrorStatus":"DestinyItemNotFound","Message":TOKEN}),
            ),
            Reply::Auth => (403, json!({"Message":TOKEN})),
            Reply::Throttled => (429, json!({"Message":KEY})),
            Reply::Uncertain { .. } => return Err(LockTransportError::Indeterminate),
        };
        let bytes = response.to_string().into_bytes();
        Ok(LockHttpResponse::new(
            status,
            Some(bytes.len() as u64),
            Some("application/json".into()),
            Box::new(Cursor::new(bytes)),
        ))
    }
}
struct TestClock;
impl Clock for TestClock {
    fn now(&self) -> Result<Timestamp, AppError> {
        Ok(Timestamp::new(1))
    }
}

#[test]
fn lock_and_unlock_preserve_dispatch_outcome_independently_of_observed_state() {
    for locked in [true, false] {
        for (reply, expected, lands) in [
            (Reply::Applied, LockOutcome::Applied, true),
            (
                Reply::Refused,
                LockOutcome::Rejected(LockRejection::ItemNotFound),
                false,
            ),
            (Reply::Auth, LockOutcome::AuthenticationRefused, false),
            (Reply::Throttled, LockOutcome::Transient, false),
            (
                Reply::Uncertain { lands: true },
                LockOutcome::Uncertain,
                true,
            ),
            (
                Reply::Uncertain { lands: false },
                LockOutcome::Uncertain,
                false,
            ),
        ] {
            let mut profile: Value =
                serde_json::from_slice(include_bytes!("fixtures/profile-complete.json")).unwrap();
            let item = profile["Response"]["profileInventory"]["data"]["items"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|item| item["itemInstanceId"] == ITEM)
                .unwrap();
            item["state"] = json!(if locked { 0 } else { 1 });
            let world = Rc::new(RefCell::new(World {
                profile,
                reply,
                calls: 0,
                debug: Vec::new(),
            }));
            let membership = DestinyMembership::new(
                MembershipType::new(3),
                MembershipId::new("4611686018484639825"),
            )
            .unwrap();
            let decode = || {
                decode_profile(
                    &serde_json::to_vec(&world.borrow().profile).unwrap(),
                    &membership,
                    DefinitionVersion::new("lock-journey-v1"),
                    Timestamp::new(1),
                )
                .unwrap()
            };
            let mut store = SqliteStore::open_in_memory().unwrap();
            let before = store.replace_current(decode()).unwrap();
            let mut gateway = BungieLockGateway::new(
                Http(world.clone()),
                TestClock,
                StaticCredentialSource::new(AccessCredential {
                    refresh_grant: None,
                    material: CredentialMaterial::from_bytes(TOKEN.as_bytes().to_vec()),
                    metadata: AccessCredentialMetadata {
                        expires_at: Timestamp::new(100),
                    },
                }),
                LockApiKey::new(KEY).unwrap(),
            )
            .unwrap();
            let request = SetLockStateRequest::new(
                membership.clone(),
                ItemInstanceId::new(ITEM),
                CharacterId::new(CHARACTER),
                locked,
            )
            .unwrap();
            let outcome = gateway.set_lock_state(&request).unwrap();
            assert_eq!(outcome, expected);
            assert_eq!(
                world.borrow().calls,
                1,
                "never retry, including uncertainty"
            );
            let after = store.replace_current(decode()).unwrap();
            let item = after
                .items
                .iter()
                .find(|item| item.instance_id.as_str() == ITEM)
                .unwrap();
            assert_eq!(item.locked, if lands { locked } else { !locked });
            assert_eq!(store.load_current().unwrap().unwrap(), after);
            assert_eq!(after.metadata.revision > before.metadata.revision, lands);
            let debug = format!(
                "{gateway:?} {request:?} {outcome:?} {:?}",
                world.borrow().debug
            );
            for secret in [KEY, TOKEN, ITEM, CHARACTER, "4611686018484639825"] {
                assert!(!debug.contains(secret));
            }
        }
    }
}
