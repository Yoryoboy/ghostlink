use destiny_core::build::*;
use destiny_core::{ItemHash, ItemInstanceId};

#[test]
fn build_bounds_and_socket_ordering() {
    let mut build = BuildDefinition {
        id: "test".into(),
        ..BuildDefinition::default()
    };
    let mut item = BuildItem {
        instance_id: Some(ItemInstanceId::new("123")),
        hash: ItemHash::new(42),
        amount: None,
        socket_overrides: vec![(3, ItemHash::new(30)), (1, ItemHash::new(10))],
        crafted_date: None,
    };
    build.equipped.push(item.clone());
    assert!(build.validate().is_err());
    item.normalize();
    build.equipped = vec![item];
    assert!(build.validate().is_ok());
    build.name = "x".repeat(MAX_BUILD_NAME_BYTES + 1);
    assert!(build.validate().is_err());
    build.name.clear();
    build.equipped[0]
        .socket_overrides
        .push((3, ItemHash::new(40)));
    assert!(build.validate().is_err());
}

#[test]
fn build_collection_and_hash_bounds() {
    let mut build = BuildDefinition {
        id: "bounded".into(),
        ..BuildDefinition::default()
    };
    let item = BuildItem {
        instance_id: None,
        hash: ItemHash::new(i64::from(u32::MAX)),
        amount: Some(u32::MAX),
        socket_overrides: vec![],
        crafted_date: Some(u64::MAX),
    };
    build.equipped = vec![item.clone(); MAX_BUILD_ITEMS];
    assert!(build.validate().is_ok());
    build.unequipped.push(item);
    assert!(build.validate().is_err());
    build.unequipped.clear();
    build.equipped.truncate(1);
    build.equipped[0].hash = ItemHash::new(-1);
    assert!(build.validate().is_err());
    build.equipped[0].hash = ItemHash::new(1);
    build.equipped[0].socket_overrides = vec![(MAX_BUILD_SOCKETS as u16, ItemHash::new(1))];
    assert!(build.validate().is_err());
    build.equipped.clear();
    build.parameters = Some(BuildParameters {
        mods: Some(vec![ItemHash::new(1); MAX_BUILD_PARAMETERS + 1]),
        ..BuildParameters::default()
    });
    assert!(build.validate().is_err());
    build.parameters = None;
    build.class_type = 4;
    assert!(build.validate().is_err());
}

#[test]
fn constraints_normalize_like_dim() {
    assert_eq!(
        StatConstraint::normalize(1, None, None, Some(12), Some(8)),
        StatConstraint {
            stat_hash: 1,
            min_stat: Some(80),
            max_stat: Some(80)
        }
    );
    assert_eq!(
        StatConstraint::normalize(1, Some(-10), Some(300), None, None),
        StatConstraint {
            stat_hash: 1,
            min_stat: Some(0),
            max_stat: Some(200)
        }
    );
    assert_eq!(
        StatConstraint::normalize(1, Some(150), None, Some(1), Some(5)).min_stat,
        Some(150)
    );
}
