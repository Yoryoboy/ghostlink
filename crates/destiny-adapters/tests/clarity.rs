use destiny_adapters::{
    clarity::{decode_descriptions, decode_version},
    sqlite::SqliteStore,
};
use destiny_core::{ClarityInsight, ClarityRepository, Timestamp};

#[test]
fn version_retains_number_lexeme() {
    assert_eq!(
        decode_version(br#"{"descriptions":2.062300e+0}"#).unwrap(),
        "2.062300e+0"
    );
    for bad in [
        r#"{"descriptions":null}"#,
        r#"{"descriptions":"2"}"#,
        r#"{"descriptions":-1}"#,
    ] {
        assert!(decode_version(bad.as_bytes()).is_err());
    }
}

#[test]
fn verbatim_and_entry_rejections() {
    let text = "  [Stasis] [enh]\n• 🡅 ?\r\t ";
    let fixture = serde_json::json!({
        "1": {"hash":1,"name":"Perk","type":"Trait","itemHash":8,"itemName":"Gun","descriptions":{"en":text}},
        "2": {"hash":3,"name":"Bad","type":"Trait","descriptions":{"en":"bad"}},
        "03": {"hash":3,"name":"Bad","type":"Trait","descriptions":{"en":"bad"}},
        "4": {"hash":4,"descriptions":{"en":null}},
        "5": {"hash":5,"descriptions":{"en":" \n"}},
        "6": {"hash":6,"name":"Bad","type":"Trait","descriptions":{"en":"bad\u{0001}"}},
        "7": {"hash":7,"name":"Bad","type":"Trait","descriptions":{"en":"x".repeat(8193)}}
    });
    let batch = decode_descriptions(&serde_json::to_vec(&fixture).unwrap()).unwrap();
    assert_eq!(batch.skipped, 6);
    assert_eq!(batch.entries.len(), 1);
    assert_eq!(batch.entries[0].text.as_bytes(), text.as_bytes());
    assert_eq!(batch.entries[0].item_hash, Some(8));
}

#[test]
fn sync_skips_unchanged_download_and_isolates_failure() {
    use destiny_core::{AppError, ClarityBatch, ClarityGateway, ErrorCategory};
    struct Gateway {
        downloads: usize,
        fail: bool,
    }
    impl ClarityGateway for Gateway {
        fn version(&mut self) -> Result<String, AppError> {
            if self.fail {
                return Err(AppError::new(ErrorCategory::ProviderUnavailable));
            }
            Ok("2.0623".into())
        }
        fn descriptions(&mut self) -> Result<ClarityBatch, AppError> {
            self.downloads += 1;
            Ok(ClarityBatch {
                entries: vec![],
                skipped: 2,
            })
        }
    }
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut gateway = Gateway {
        downloads: 0,
        fail: false,
    };
    assert_eq!(
        destiny_app::clarity::sync_clarity(&mut gateway, &mut store, Timestamp::new(1)).status,
        "updated"
    );
    assert_eq!(
        destiny_app::clarity::sync_clarity(&mut gateway, &mut store, Timestamp::new(2)).status,
        "unchanged"
    );
    assert_eq!(gateway.downloads, 1);
    let metadata = store.clarity_metadata().unwrap().unwrap();
    assert_eq!(metadata.checked_at, Timestamp::new(2));
    assert_eq!(metadata.updated_at, Timestamp::new(1));
    gateway.fail = true;
    assert_eq!(
        destiny_app::clarity::sync_clarity(&mut gateway, &mut store, Timestamp::new(3)).status,
        "unavailable"
    );
    assert_eq!(store.clarity_metadata().unwrap().unwrap(), metadata);
}

#[test]
fn adapter_requests_fixed_resources_and_skips_unchanged_body() {
    use destiny_adapters::clarity::{ClarityAdapter, ClarityResource, ClarityTransport};
    use destiny_core::AppError;
    #[derive(Default)]
    struct Transport(Vec<ClarityResource>);
    impl ClarityTransport for Transport {
        fn get(&mut self, resource: ClarityResource) -> Result<Vec<u8>, AppError> {
            self.0.push(resource);
            Ok(match resource {
                ClarityResource::Versions => br#"{"descriptions":2.0623}"#.to_vec(),
                ClarityResource::Descriptions => {
                    br#"{"1":{"hash":1,"name":"Perk","type":"Trait","descriptions":{"en":" ? "}}}"#
                        .to_vec()
                }
            })
        }
    }
    let mut gateway = ClarityAdapter(Transport::default());
    let mut store = SqliteStore::open_in_memory().unwrap();
    for status in ["updated", "unchanged"] {
        let report =
            destiny_app::clarity::sync_clarity(&mut gateway, &mut store, Timestamp::new(1));
        assert_eq!(report.status, status);
        assert_eq!(report.version.as_deref(), Some("2.0623"));
        assert_eq!(report.entries_stored, 1);
    }
    assert_eq!(
        gateway.0.0,
        [
            ClarityResource::Versions,
            ClarityResource::Descriptions,
            ClarityResource::Versions
        ]
    );
}

#[test]
fn decode_bounds_and_missing_english() {
    assert!(decode_version(&vec![b' '; 4097]).is_err());
    assert!(decode_descriptions(&vec![b' '; 16 * 1024 * 1024 + 1]).is_err());
    let entries: serde_json::Map<String, serde_json::Value> = (0..20_001)
        .map(|hash| (hash.to_string(), serde_json::Value::Null))
        .collect();
    assert!(decode_descriptions(&serde_json::to_vec(&entries).unwrap()).is_err());
    let batch = decode_descriptions(
        br#"{"1":{"hash":1,"descriptions":{}},"2":{"hash":2,"descriptions":{"en":17}}}"#,
    )
    .unwrap();
    assert_eq!(batch.skipped, 2);
    assert!(batch.entries.is_empty());
}

#[test]
fn migration_and_atomic_publish() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.migrate_to(14).unwrap();
    store.migrate_to(15).unwrap();
    let entry = ClarityInsight {
        hash: 1,
        name: "Perk".into(),
        kind: "Trait".into(),
        item_hash: None,
        item_name: None,
        text: " \n🡅? ".into(),
    };
    store
        .publish_clarity("2.0623", std::slice::from_ref(&entry), Timestamp::new(10))
        .unwrap();
    assert_eq!(store.clarity_by_hash(1).unwrap(), Some(entry.clone()));
    assert_eq!(store.clarity_by_name("Perk").unwrap(), vec![entry.clone()]);
    assert!(
        store
            .publish_clarity("new", &[entry.clone(), entry.clone()], Timestamp::new(11))
            .is_err()
    );
    assert_eq!(store.clarity_metadata().unwrap().unwrap().version, "2.0623");
    assert_eq!(store.clarity_by_hash(1).unwrap(), Some(entry.clone()));
    let second = ClarityInsight {
        hash: 2,
        ..entry.clone()
    };
    store
        .publish_clarity("next", &[entry.clone(), second.clone()], Timestamp::new(12))
        .unwrap();
    assert_eq!(
        store.clarity_by_name("Perk").unwrap(),
        vec![entry, second.clone()]
    );
    assert!(store.clarity_by_name("perk").unwrap().is_empty());
    store
        .publish_clarity("last", &[second], Timestamp::new(13))
        .unwrap();
    assert!(store.clarity_by_hash(1).unwrap().is_none());
    assert_eq!(store.clarity_metadata().unwrap().unwrap().entries, 1);
    store.migrate_to(14).unwrap();
    assert!(store.clarity_metadata().is_err());
    store.migrate_to(15).unwrap();
    assert!(store.clarity_metadata().unwrap().is_none());
}
