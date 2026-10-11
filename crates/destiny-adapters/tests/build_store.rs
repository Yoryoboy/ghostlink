use destiny_adapters::{
    dim_loadout::{DimBuildCodec, export_dim_loadout, parse_dim_loadout},
    sqlite::SqliteStore,
};
use destiny_app::{
    builds::{BuildError, BuildSelector, BuildService},
    test_support::InMemoryBuildRepository,
};
use destiny_core::{
    DestinyMembership, MembershipId, MembershipType, Timestamp, build::*, ports::BuildRepository,
};

fn scope(id: &str) -> DestinyMembership {
    DestinyMembership::new(MembershipType::new(3), MembershipId::new(id)).unwrap()
}
fn definition(id: &str) -> BuildDefinition {
    parse_dim_loadout(&format!(r#"{{"id":"{id}","name":"Raid","classType":1,"equipped":[{{"id":"123","hash":42,"socketOverrides":{{"7":99}}}}],"unequipped":[],"parameters":{{"future":{{"b":2,"a":1}},"mods":[7,7]}}}}"#)).unwrap()
}
fn parity(repository: &mut impl BuildRepository) {
    let a = scope("1");
    let b = scope("2");
    let build = definition("a");
    repository
        .save_build(&a, &build, BuildSource::Import, Timestamp::new(10))
        .unwrap();
    assert_eq!(
        repository.get_build(&a, "a").unwrap().unwrap().definition,
        build
    );
    assert!(repository.list_builds(&b).unwrap().is_empty());
    repository
        .save_build(
            &a,
            &definition("b"),
            BuildSource::Equipped,
            Timestamp::new(11),
        )
        .unwrap();
    assert!(
        matches!(repository.find_build_by_name(&a, "  rAiD ").unwrap(), destiny_core::Resolution::Ambiguous(v) if v.len() == 2)
    );
    let mut changed = build.clone();
    changed.name = "Updated".into();
    repository
        .save_build(&a, &changed, BuildSource::Import, Timestamp::new(20))
        .unwrap();
    let stored = repository.get_build(&a, "a").unwrap().unwrap();
    assert_eq!(stored.summary.created_at, Timestamp::new(10));
    assert_eq!(stored.summary.updated_at, Timestamp::new(20));
    let mut edge = build.clone();
    edge.id = "edge".into();
    edge.created_at = Some(u64::MAX);
    edge.last_updated_at = Some(0);
    let parameters = edge.parameters.as_mut().unwrap();
    parameters.extra = Some("{}".into());
    parameters.stat_constraints = Some(vec![StatConstraint {
        stat_hash: 1,
        min_stat: Some(100),
        max_stat: Some(20),
    }]);
    repository
        .save_build(&a, &edge, BuildSource::Import, Timestamp::new(u64::MAX))
        .unwrap();
    assert_eq!(
        repository
            .get_build(&a, "edge")
            .unwrap()
            .unwrap()
            .definition,
        edge
    );
    let mut invalid = build;
    invalid.class_type = 4;
    assert!(
        repository
            .save_build(&a, &invalid, BuildSource::Import, Timestamp::new(30))
            .is_err()
    );
    assert_eq!(repository.get_build(&a, "a").unwrap().unwrap(), stored);
    repository
        .save_build(
            &b,
            &definition("a"),
            BuildSource::InGameLoadout,
            Timestamp::new(1),
        )
        .unwrap();
    assert!(repository.delete_build(&a, "a").unwrap());
    assert!(!repository.delete_build(&a, "a").unwrap());
    assert!(repository.get_build(&b, "a").unwrap().is_some());
}
#[test]
fn fake_sqlite_repository_parity() {
    parity(&mut InMemoryBuildRepository::default());
    parity(&mut SqliteStore::open_in_memory().unwrap());
}
#[test]
fn migration_is_additive_and_refuses_lossy_downgrade() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    assert_eq!(store.schema_version().unwrap(), 23);
    store.migrate_to(22).unwrap();
    store.migrate_to(23).unwrap();
    store
        .save_build(
            &scope("1"),
            &definition("a"),
            BuildSource::Import,
            Timestamp::new(1),
        )
        .unwrap();
    assert!(store.migrate_to(22).is_err());
    assert_eq!(store.schema_version().unwrap(), 23);
    assert!(store.get_build(&scope("1"), "a").unwrap().is_some());
}
#[test]
fn import_store_export_and_name_ambiguity() {
    let mut service = BuildService::new(SqliteStore::open_in_memory().unwrap(), DimBuildCodec);
    let scope = scope("1");
    let json = export_dim_loadout(&definition("original")).unwrap();
    let stored = service.import(&scope, &json, Timestamp::new(1)).unwrap();
    assert_ne!(stored.definition.id, "original");
    let selector = BuildSelector::Id(stored.definition.id.clone());
    let exported = service.export(&scope, &selector).unwrap();
    assert_eq!(parse_dim_loadout(&exported).unwrap(), stored.definition);
    let url = url::Url::parse_with_params(
        "https://app.destinyitemmanager.com/loadouts",
        &[("loadout", json.as_str())],
    )
    .unwrap();
    service
        .import(&scope, url.as_str(), Timestamp::new(2))
        .unwrap();
    assert!(
        matches!(service.show(&scope, &BuildSelector::Name("raid".into())), Err(BuildError::AmbiguousName(v)) if v.len() == 2)
    );
    assert_eq!(service.list(&scope).unwrap().len(), 2);
    assert!(matches!(
        service.import(&scope, "https://dim.gg/abcdefg/test", Timestamp::new(3)),
        Err(BuildError::Codec(
            destiny_adapters::dim_loadout::DimLoadoutError::ShareLinkRequiresOnlineImport
        ))
    ));
    assert!(service.delete(&scope, &selector).unwrap());
    assert!(matches!(
        service.show(&scope, &selector),
        Err(BuildError::NotFound)
    ));
    let by_name = BuildSelector::Name("RAID".into());
    assert!(service.show(&scope, &by_name).is_ok());
    assert!(service.delete(&scope, &by_name).unwrap());
    assert!(service.list(&scope).unwrap().is_empty());
    assert!(service.import(&scope, "{}", Timestamp::new(4)).is_err());
    assert!(service.list(&scope).unwrap().is_empty());
}
