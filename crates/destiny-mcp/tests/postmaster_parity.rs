//! Exact CLI/MCP logical JSON parity; no production CLI dependency.
use destiny_app::*;
use destiny_mcp::service::postmaster_output;

#[test]
fn postmaster_cli_json_parity_preserves_rows_and_metadata() {
    let stack = PostmasterStack {
        item_hash: ItemHash::new(4_294_967_295),
        bucket_hash: BucketHash::new(215593132),
        quantity: 7,
        name: None,
    };
    let mut result = PostmasterResult {
        metadata: QueryMetadata {
            sync: SyncMetadata {
                dataset: Dataset::ProfileInventory,
                revision: SyncRevision::new(9),
                checked_at: Timestamp::new(10000),
                source: SourceMarker::Profile,
                definition_version: DefinitionVersion::new("private-definition-marker"),
            },
            freshness: FreshnessStatus::Fresh,
        },
        profile_currencies_coverage: ProfileCurrenciesCoverage::NotObserved,
        characters: vec![
            CharacterPostmaster {
                character_id: CharacterId::new("123"),
                class: Some("Warlock".into()),
                instances: vec![PostmasterInstance {
                    instance_id: ItemInstanceId::new("456"),
                    item_hash: ItemHash::new(1),
                    name: Some("Name\n\"quoted\"\u{7}".into()),
                    quantity: 1,
                }],
                stacks: vec![stack.clone(), stack],
            },
            CharacterPostmaster {
                character_id: CharacterId::new("789"),
                class: None,
                instances: vec![],
                stacks: vec![],
            },
        ],
    };
    for coverage in [
        ProfileCurrenciesCoverage::NotObserved,
        ProfileCurrenciesCoverage::Unavailable,
        ProfileCurrenciesCoverage::Available,
    ] {
        result.profile_currencies_coverage = coverage;
        for freshness in [
            FreshnessStatus::Fresh,
            FreshnessStatus::Stale,
            FreshnessStatus::Missing,
            FreshnessStatus::InvalidSource,
        ] {
            result.metadata.freshness = freshness;
            let output = postmaster_output(&result).into_call_tool_result();
            let expected = destiny_cli::postmaster_output::postmaster_json(&result);
            assert_eq!(output.structured_content.as_ref(), Some(&expected));
            let encoded = serde_json::to_string(&output).unwrap();
            assert!(!encoded.contains("private-definition-marker"));
            assert!(!encoded.contains('\u{7}'));
            assert_eq!(
                expected["characters"][0]["stacks"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            assert!(
                expected["characters"][0]["instances"][0]
                    .get("bucket_hash")
                    .is_none()
            );
            assert!(
                expected["characters"][0]["stacks"][0]
                    .get("instance_id")
                    .is_none()
            );
            assert!(expected["characters"][1]["class"].is_null());
        }
    }
    result.characters.clear();
    assert_eq!(
        postmaster_output(&result)
            .into_call_tool_result()
            .structured_content
            .unwrap(),
        destiny_cli::postmaster_output::postmaster_json(&result)
    );
}

#[test]
fn postmaster_selector_matches_optional_read_convention() {
    use destiny_mcp::schema::GetPostmasterArgs;
    for arguments in [
        serde_json::json!({}),
        serde_json::json!({"character": null}),
    ] {
        assert_eq!(
            serde_json::from_value::<GetPostmasterArgs>(arguments)
                .unwrap()
                .into_request()
                .unwrap(),
            None
        );
    }
    for value in ["123", "wArLoCk"] {
        let args: GetPostmasterArgs =
            serde_json::from_value(serde_json::json!({"character": value})).unwrap();
        assert_eq!(args.into_request().unwrap(), Some(CharacterId::new(value)));
    }
}
