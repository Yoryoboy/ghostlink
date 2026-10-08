use clap::Parser;
use destiny_app::{
    CharacterId, CharacterLoadoutsView, ItemHash, ItemInstanceId, LoadoutItemView, LoadoutPlugView,
    LoadoutView, LoadoutsResult,
};
use destiny_cli::{
    Cli,
    command::ReadRequest,
    output::OutputMode,
    runner::{ReadFreshness, dispatch_loadout, dispatch_loadouts},
};

struct Fixture {
    calls: usize,
}
fn result() -> LoadoutsResult {
    LoadoutsResult {
        characters: vec![CharacterLoadoutsView {
            character_id: CharacterId::new("1"),
            class: Some("Hunter".into()),
            loadouts: vec![LoadoutView {
                slot: 3,
                name: Some("PvE".into()),
                name_hash: 7,
                color_hash: 8,
                icon_hash: 9,
                items: vec![LoadoutItemView {
                    instance_id: ItemInstanceId::new("123"),
                    item_hash: Some(ItemHash::new(1)),
                    name: Some("Weapon".into()),
                    item_type: Some("Auto Rifle".into()),
                    bucket_hash: Some(1498876634),
                    element: None,
                    missing: false,
                    plugs: vec![LoadoutPlugView {
                        socket_index: 2,
                        plug_hash: ItemHash::new(10),
                        name: Some("Perk".into()),
                        description: Some("Official text".into()),
                        clarity: Some("Research".into()),
                        clarity_match: None,
                        subclass_kind: None,
                        empty: false,
                    }],
                }],
            }],
        }],
    }
}
impl destiny_app::LoadoutOperations for Fixture {
    fn loadouts(
        &mut self,
        character: Option<&CharacterId>,
        policy: destiny_app::FreshnessPolicy,
        now: destiny_app::Timestamp,
    ) -> Result<LoadoutsResult, destiny_app::AppError> {
        self.calls += 1;
        assert_eq!(character.map(CharacterId::as_str), Some("hunter"));
        assert_eq!(policy.ttl_seconds, 300);
        assert_eq!(now, destiny_app::Timestamp::new(1000));
        Ok(result())
    }
}
fn freshness() -> ReadFreshness {
    ReadFreshness {
        policy: destiny_app::FreshnessPolicy { ttl_seconds: 300 },
        now: destiny_app::Timestamp::new(1000),
    }
}
#[test]
fn positional_detail_slot_name_and_summary_route_once_with_exact_shared_json() {
    for selector in [None, Some("3"), Some(" pVe ")] {
        let mut args = vec!["ghostlink", "loadouts", "--character", "hunter", "--json"];
        if let Some(selector) = selector {
            args.push(selector);
        }
        let cli = Cli::try_parse_from(args).unwrap();
        let ReadRequest::Loadouts {
            character_id,
            loadout,
            detail,
        } = cli.into_read_request().unwrap()
        else {
            panic!("loadouts")
        };
        let mut service = Fixture { calls: 0 };
        let output = match loadout.as_ref() {
            Some(loadout) => dispatch_loadout(
                &mut service,
                character_id.as_ref(),
                loadout,
                OutputMode::Json,
                freshness(),
            ),
            None => dispatch_loadouts(
                &mut service,
                character_id.as_ref(),
                detail,
                OutputMode::Json,
                freshness(),
            ),
        };
        assert_eq!(service.calls, 1);
        assert_eq!(output.exit_code, 0);
        assert!(output.stderr.is_empty());
        let actual: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
        let expected = if let Some(selector) = loadout {
            let selected = destiny_app::loadouts::select_loadout(&result(), &selector).unwrap();
            let text = destiny_cli::output::render_loadout(OutputMode::Human, &selected);
            assert!(text.contains("Official text"));
            assert!(text.contains("Clarity:"));
            destiny_output::loadouts::loadout_json(&selected)
        } else {
            destiny_output::loadouts::loadouts_json(&result())
        };
        assert_eq!(actual, expected);
    }
}
#[test]
fn detail_selector_errors_are_typed_and_sanitized() {
    for (selector, code) in [("1", "empty_slot"), ("Pv", "unknown_loadout_name")] {
        let selector = destiny_app::loadouts::parse_loadout_selector(selector).unwrap();
        let mut service = Fixture { calls: 0 };
        let output = dispatch_loadout(
            &mut service,
            Some(&CharacterId::new("hunter")),
            &selector,
            OutputMode::Json,
            freshness(),
        );
        assert_eq!(output.exit_code, 41);
        assert!(output.stdout.is_empty());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output.stderr).unwrap()["code"],
            code
        );
    }
    for value in ["0", "13", "256", " ", "bad\u{7}"] {
        assert!(
            Cli::try_parse_from(["ghostlink", "loadouts", value, "--json"])
                .unwrap()
                .into_read_request()
                .is_err()
        );
    }
    assert!(Cli::try_parse_from(["ghostlink", "loadouts", "3", "extra"]).is_err());
}
