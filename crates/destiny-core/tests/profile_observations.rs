use destiny_core::*;

fn input() -> InventorySnapshotInput {
    InventorySnapshotInput {
        membership_id: MembershipId::new("123"),
        membership_type: MembershipType::new(3),
        definition_version: DefinitionVersion::new("synthetic"),
        checked_at: Timestamp::new(42),
        source: SourceMarker::Profile,
        characters: vec![Character {
            id: CharacterId::new("owner"),
            name: Some("Synthetic".into()),
            class: None,
            race: None,
            gender: None,
            light: None,
            date_last_played: Some(Timestamp::new(7)),
        }],
        items: vec![ItemInstance {
            instance_id: ItemInstanceId::new("opaque-instance"),
            item_hash: ItemHash::new(42),
            location: ItemLocation::Postmaster(CharacterId::new("owner")),
            quantity: 3,
            equipped: false,
            locked: true,
            power: Some(10),
            perks: vec![],
            plugs: vec![],
            transfer_status: None,
            available_plugs: vec![],
            stats: vec![],
        }],
        stacks: vec![],
        profile_currencies_coverage: ProfileCurrenciesCoverage::Unavailable,
        plug_set_entries: vec![],
    }
}

fn observation(item: &ItemInstance, bucket: i64) -> InstancedBucketObservation {
    InstancedBucketObservation::new(
        item.instance_id.clone(),
        BucketHash::new(bucket),
        item.location.clone(),
    )
    .unwrap()
}

#[test]
fn minted_instants_order_precisely_without_changing_input_or_observations() {
    let first = ProfileMintedAt::new(-1, 999_999_999).unwrap();
    let next = ProfileMintedAt::new(0, 0).unwrap();
    assert!(first < next);
    assert!(next < ProfileMintedAt::new(0, 1).unwrap());
    assert_eq!(
        ProfileMintedAt::new(0, 1_000_000_000)
            .unwrap_err()
            .category(),
        ErrorCategory::ProviderValidation
    );
    let original = ProfileAcquisition::new(input(), None).unwrap();
    assert_eq!(original.minted_at(), None);
    let minted = original.clone().with_minted_at(Some(first));
    assert_eq!(minted.minted_at(), Some(&first));
    assert_eq!(minted.into_parts(), original.into_parts());
}

#[test]
fn complete_pair_preserves_all_input_and_same_hash_instances() {
    let mut snapshot = input();
    let mut second = snapshot.items[0].clone();
    second.instance_id = ItemInstanceId::new("another-opaque-id");
    snapshot.items.push(second);
    let stack = StackObservation::new(
        ItemHash::new(42),
        BucketHash::new(9),
        2,
        StackContainer::ProfileCurrencies,
    )
    .unwrap();
    snapshot.stacks = vec![stack.clone(), stack];
    snapshot.profile_currencies_coverage = ProfileCurrenciesCoverage::Available;
    let rows: Vec<_> = snapshot
        .items
        .iter()
        .map(|item| observation(item, 7))
        .collect();
    let acquisition = ProfileAcquisition::new(snapshot.clone(), Some(rows.clone())).unwrap();
    assert_eq!(acquisition.input(), &snapshot);
    assert_eq!(acquisition.observations(), Some(rows.as_slice()));
    assert_ne!(rows[0].instance_id(), rows[1].instance_id());
    assert_eq!(rows[0].bucket_hash(), BucketHash::new(7));
    assert_eq!(rows[0].location(), &snapshot.items[0].location);
    assert_eq!(acquisition.into_parts(), (snapshot, Some(rows)));
}

#[test]
fn unavailable_is_not_available_empty_and_stacks_are_independent() {
    let snapshot = input();
    let unavailable = ProfileAcquisition::new(snapshot.clone(), None).unwrap();
    assert_eq!(unavailable.observations(), None);
    assert!(ProfileAcquisition::new(snapshot.clone(), Some(vec![])).is_err());
    let mut empty = snapshot;
    empty.items.clear();
    empty.stacks.push(
        StackObservation::new(
            ItemHash::new(42),
            BucketHash::new(0),
            1,
            StackContainer::ProfileInventory,
        )
        .unwrap(),
    );
    let available = ProfileAcquisition::new(empty.clone(), Some(vec![])).unwrap();
    assert_eq!(available.observations(), Some([].as_slice()));
    assert_eq!(available.input(), &empty);
}

#[test]
fn missing_extra_duplicate_and_association_mismatches_are_rejected() {
    let mut snapshot = input();
    let mut second = snapshot.items[0].clone();
    second.instance_id = ItemInstanceId::new("second");
    snapshot.items.push(second);
    let valid = observation(&snapshot.items[0], 7);
    let mut other = snapshot.items[0].clone();
    other.instance_id = ItemInstanceId::new("extra");
    let extra = observation(&other, 7);
    for rows in [
        vec![],
        vec![valid.clone()],
        vec![valid.clone(), extra.clone()],
        vec![valid.clone(), valid],
        vec![extra],
    ] {
        assert_eq!(
            ProfileAcquisition::new(snapshot.clone(), Some(rows))
                .unwrap_err()
                .category(),
            ErrorCategory::ProviderValidation
        );
    }
    let second_row = observation(&snapshot.items[1], 7);
    let complete = vec![observation(&snapshot.items[0], 7), second_row.clone()];
    assert!(ProfileAcquisition::new(snapshot.clone(), Some(complete)).is_ok());
    for location in [
        ItemLocation::Vault,
        ItemLocation::Character(CharacterId::new("owner")),
        ItemLocation::Postmaster(CharacterId::new("other-owner")),
        ItemLocation::other("unknown"),
    ] {
        let row = InstancedBucketObservation::new(
            snapshot.items[0].instance_id.clone(),
            BucketHash::new(7),
            location,
        )
        .unwrap();
        assert!(
            ProfileAcquisition::new(snapshot.clone(), Some(vec![row, second_row.clone()])).is_err()
        );
    }
}

#[test]
fn runtime_bucket_accepts_uint32_including_zero_and_rejects_overflow() {
    let snapshot = input();
    for bucket in [0, 1, i64::from(u32::MAX)] {
        let row = observation(&snapshot.items[0], bucket);
        assert_eq!(row.bucket_hash().value(), bucket);
        assert!(ProfileAcquisition::new(snapshot.clone(), Some(vec![row])).is_ok());
    }
    for bucket in [i64::MIN, -1, i64::from(u32::MAX) + 1, i64::MAX] {
        assert_eq!(
            InstancedBucketObservation::new(
                snapshot.items[0].instance_id.clone(),
                BucketHash::new(bucket),
                ItemLocation::Vault,
            )
            .unwrap_err()
            .category(),
            ErrorCategory::ProviderValidation
        );
    }
}

#[test]
fn existing_structural_validation_applies_even_without_observations() {
    let snapshot = input();
    let mut duplicate = snapshot.clone();
    duplicate.items.push(duplicate.items[0].clone());
    let mut ownerless = snapshot.clone();
    ownerless.characters.clear();
    let mut equipped = snapshot.clone();
    equipped.items[0].equipped = true;
    let mut currency = snapshot.clone();
    currency.stacks.push(
        StackObservation::new(
            ItemHash::new(42),
            BucketHash::new(9),
            1,
            StackContainer::ProfileCurrencies,
        )
        .unwrap(),
    );
    for invalid in [duplicate, ownerless, equipped, currency] {
        let rows = invalid
            .items
            .iter()
            .map(|item| observation(item, 7))
            .collect();
        for evidence in [None, Some(rows)] {
            assert_eq!(
                ProfileAcquisition::new(invalid.clone(), evidence)
                    .unwrap_err()
                    .category(),
                ErrorCategory::ProviderValidation
            );
        }
    }
}

struct Gateway {
    calls: Vec<(DestinyMembership, ProfileSyncReason)>,
    result: Result<InventorySnapshotInput, AppError>,
}

impl ProfileGateway for Gateway {
    fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
        panic!("default observation acquisition must not resolve membership")
    }
    fn load_inventory(
        &mut self,
        _: &DestinyMembership,
    ) -> Result<InventorySnapshotInput, AppError> {
        panic!("must use the scoped loader")
    }
    fn load_inventory_scoped(
        &mut self,
        membership: &DestinyMembership,
        reason: ProfileSyncReason,
    ) -> Result<InventorySnapshotInput, AppError> {
        self.calls.push((membership.clone(), reason));
        self.result.clone()
    }
}

#[test]
fn default_gateway_forwards_one_scoped_call_without_resolution_or_retry() {
    let membership =
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("999")).unwrap();
    let mut gateway = Gateway {
        calls: vec![],
        result: Ok(input()),
    };
    let result = gateway
        .load_inventory_with_observations_scoped(&membership, ProfileSyncReason::Preflight)
        .unwrap();
    assert_eq!(
        gateway.calls,
        vec![(membership.clone(), ProfileSyncReason::Preflight)]
    );
    assert_eq!(result.observations(), None);
    assert_eq!(result.input(), &input()); // Requested membership validation remains sync's responsibility.
    gateway.calls.clear();
    let error = AppError::new(ErrorCategory::ProviderUnavailable);
    gateway.result = Err(error.clone());
    let actual = gateway
        .load_inventory_with_observations_scoped(&membership, ProfileSyncReason::Forced)
        .unwrap_err();
    assert_eq!(actual, error);
    assert_eq!(gateway.calls, vec![(membership, ProfileSyncReason::Forced)]);
}
