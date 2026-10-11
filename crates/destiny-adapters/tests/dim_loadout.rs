use destiny_adapters::dim_loadout::*;
use destiny_core::build::ExoticArmor;

const FULL: &str = include_str!("fixtures/dim_loadout/full.json");
const LEGACY: &str = include_str!("fixtures/dim_loadout/legacy.json");
const UNKNOWN: &str = include_str!("fixtures/dim_loadout/unknown.json");

#[test]
fn dim_round_trip_and_unknown_fields() {
    for json in [FULL, LEGACY, UNKNOWN] {
        let build = parse_dim_loadout(json).unwrap();
        assert_eq!(
            parse_dim_loadout(&export_dim_loadout(&build).unwrap()).unwrap(),
            build
        );
    }
    let build = parse_dim_loadout(FULL).unwrap();
    assert_eq!(
        build.parameters.unwrap().exotic_armor_hash,
        Some(ExoticArmor::Any)
    );
    assert!(build.equipped[0].instance_id.is_none());
    assert_eq!(build.equipped[0].socket_overrides[0].0, 2);
    let legacy = parse_dim_loadout(LEGACY).unwrap();
    assert_eq!(
        legacy.parameters.unwrap().stat_constraints.unwrap()[0].min_stat,
        Some(80)
    );
    assert_eq!(
        parse_dim_loadout(UNKNOWN)
            .unwrap()
            .parameters
            .unwrap()
            .extra
            .as_deref(),
        Some("{\"another\":42,\"future\":{\"a\":\"kept\",\"z\":[true,null,3]}}")
    );
}

#[test]
fn dim_url_and_fresh_import() {
    let mut url = url::Url::parse("https://app.destinyitemmanager.com/461/1/loadouts").unwrap();
    url.query_pairs_mut().append_pair("loadout", FULL);
    assert_eq!(
        parse_dim_loadout(url.as_str()).unwrap(),
        parse_dim_loadout(FULL).unwrap()
    );
    let a = import_dim_loadout(url.as_str()).unwrap();
    let b = import_dim_loadout(FULL).unwrap();
    assert_ne!(a.id, b.id);
    assert_eq!(a.id.len(), 36);
    assert_ne!(a.id, "original");
    assert_eq!(a.equipped[0].amount, Some(1));
}

#[test]
fn dim_typed_errors() {
    for input in [
        "abcdefg",
        "dim.gg/abcdefg/name",
        "https://dim.gg/abcdefg/name",
        "http://dim.gg/abcdefg",
    ] {
        assert_eq!(
            parse_dim_loadout(input),
            Err(DimLoadoutError::ShareLinkRequiresOnlineImport)
        );
    }
    for input in [
        "{}",
        "not json",
        "https://app.destinyitemmanager.com/loadouts",
        "https://app.destinyitemmanager.com/wrong?loadout=%7B%7D",
        "https://app.destinyitemmanager.com/loadouts?loadout=%ZZ",
    ] {
        assert!(parse_dim_loadout(input).is_err());
    }
    assert!(parse_dim_loadout(&FULL.replace("\"hash\":123", "\"hash\":-3")).is_err());
    assert!(parse_dim_loadout(&FULL.replace("\"10\":400", "\"02\":400")).is_err());
    assert!(
        parse_dim_loadout(&FULL.replace("\"exoticArmorHash\":-2", "\"exoticArmorHash\":-3"))
            .is_err()
    );
    assert!(
        parse_dim_loadout(
            &FULL.replace("\"assumeArmorMasterwork\":4", "\"assumeArmorMasterwork\":5")
        )
        .is_err()
    );
    assert_eq!(
        parse_dim_loadout(&"x".repeat(MAX_DIM_INPUT_BYTES + 1)),
        Err(DimLoadoutError::InputTooLarge)
    );
}

#[test]
fn legacy_clear_space_and_absent_lists() {
    let build =
        parse_dim_loadout(r#"{"id":"old","name":"Legacy","classType":3,"clearSpace":true}"#)
            .unwrap();
    let p = build.parameters.as_ref().unwrap();
    assert_eq!(p.clear_armor, Some(true));
    assert_eq!(p.clear_weapons, Some(true));
    assert_eq!(
        parse_dim_loadout(&export_dim_loadout(&build).unwrap()).unwrap(),
        build
    );
    let explicit = parse_dim_loadout(
        r#"{"id":"old","name":"Legacy","classType":3,"clearSpace":true,"parameters":{"clearArmor":false}}"#,
    )
    .unwrap();
    let p = explicit.parameters.unwrap();
    assert_eq!(p.clear_armor, Some(false));
    assert_eq!(p.clear_weapons, None);
}

#[test]
fn mixed_legacy_constraints_retain_dim_semantics() {
    for pair in [
        r#"{"statHash":1,"minStat":150,"maxTier":5}"#,
        r#"{"statHash":1,"minTier":10,"maxStat":5}"#,
    ] {
        let json = format!(
            r#"{{"id":"mixed","name":"Mixed","classType":3,"parameters":{{"statConstraints":[{pair}]}}}}"#
        );
        let build = parse_dim_loadout(&json).unwrap();
        assert_eq!(
            parse_dim_loadout(&export_dim_loadout(&build).unwrap()).unwrap(),
            build
        );
    }
}

#[test]
fn export_rejects_invalid_opaque_parameters() {
    let mut build = parse_dim_loadout(UNKNOWN).unwrap();
    for extra in ["[]", "not json", r#"{"mods":[1]}"#] {
        build.parameters.as_mut().unwrap().extra = Some(extra.into());
        assert_eq!(export_dim_loadout(&build), Err(DimLoadoutError::Validation));
    }
}
