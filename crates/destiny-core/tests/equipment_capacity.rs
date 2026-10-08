use destiny_core::*;

const BUCKETS: [i64; 8] = [
    1498876634, 2465295065, 953998645, 3448274439, 3551918588, 14239492, 20886954, 1585787867,
];
fn bucket() -> BucketHash {
    BucketHash::new(BUCKETS[0])
}
fn owner() -> CharacterId {
    CharacterId::new("owner")
}
fn entries() -> Vec<(BucketHash, usize)> {
    BUCKETS
        .into_iter()
        .map(|hash| (BucketHash::new(hash), 10))
        .collect()
}
fn policy() -> EquipmentCapacityPolicy {
    EquipmentCapacityPolicy::new(DefinitionVersion::new("v1"), "artifact", entries()).unwrap()
}
fn qualified() -> QualifiedEquipmentCapacityPolicy {
    policy()
        .qualify(&EquipmentCapacityEvidence::new(DefinitionVersion::new("v1"), "artifact").unwrap())
        .unwrap()
}
fn acquisition(count: usize) -> ProfileAcquisition {
    let characters = [owner(), CharacterId::new("other")]
        .into_iter()
        .map(|id| Character {
            id,
            name: None,
            class: None,
            race: None,
            gender: None,
            light: None,
            date_last_played: None,
        })
        .collect();
    let items = (0..count)
        .map(|id| ItemInstance {
            instance_id: ItemInstanceId::new(id.to_string()),
            item_hash: ItemHash::new(42),
            location: ItemLocation::Character(owner()),
            quantity: 1,
            equipped: id == 0,
            locked: false,
            power: None,
            perks: vec![],
            plugs: vec![],
            available_plugs: vec![],
            stats: vec![],
        })
        .collect::<Vec<_>>();
    let rows = items
        .iter()
        .map(|item| {
            InstancedBucketObservation::new(
                item.instance_id.clone(),
                bucket(),
                item.location.clone(),
            )
            .unwrap()
        })
        .collect();
    ProfileAcquisition::new(
        InventorySnapshotInput {
            membership_id: MembershipId::new("123"),
            membership_type: MembershipType::new(3),
            definition_version: DefinitionVersion::new("v1"),
            checked_at: Timestamp::new(42),
            source: SourceMarker::Profile,
            characters,
            items,
            stacks: vec![],
            profile_currencies_coverage: ProfileCurrenciesCoverage::Unavailable,
            plug_set_entries: vec![],
        },
        Some(rows),
    )
    .unwrap()
}
fn prove(pair: &ProfileAcquisition) -> Result<EquipmentCapacityProof, CapacityRefusal> {
    qualified().prove(pair, &owner(), Some(bucket()))
}

#[test]
fn policy_requires_exact_supported_set_positive_capacity_and_identity() {
    let valid = entries();
    assert_eq!(policy().definition_version().as_str(), "v1");
    assert_eq!(policy().bucket_artifact_identity(), "artifact");
    for hash in BUCKETS {
        assert_eq!(policy().capacity(BucketHash::new(hash)), Some(10));
    }
    assert_eq!(policy().capacity(BucketHash::new(0)), None);
    let mut missing = valid.clone();
    missing.pop();
    let mut extra = valid.clone();
    extra.push((BucketHash::new(0), 10));
    let mut unsupported = valid.clone();
    unsupported[0].0 = BucketHash::new(0);
    let mut duplicate = valid.clone();
    duplicate[0] = duplicate[1];
    let mut zero = valid.clone();
    zero[0].1 = 0;
    for invalid in [missing, extra, unsupported, duplicate, zero] {
        assert_eq!(
            EquipmentCapacityPolicy::new(DefinitionVersion::new("v1"), "artifact", invalid)
                .unwrap_err()
                .category(),
            ErrorCategory::ProviderValidation
        );
    }
    for (version, artifact) in [("", "artifact"), ("v1", "")] {
        assert!(
            EquipmentCapacityPolicy::new(DefinitionVersion::new(version), artifact, valid.clone())
                .is_err()
        );
        assert!(EquipmentCapacityEvidence::new(DefinitionVersion::new(version), artifact).is_err());
    }
}

#[test]
fn qualification_requires_both_exact_identities() {
    let matching =
        EquipmentCapacityEvidence::new(DefinitionVersion::new("v1"), "artifact").unwrap();
    assert!(policy().qualify(&matching).is_ok());
    for (version, artifact, reason) in [
        ("v2", "artifact", CapacityPolicyMismatch::VersionMismatch),
        (
            "v1",
            "other-artifact",
            CapacityPolicyMismatch::ArtifactMismatch,
        ),
        (
            "v2",
            "other-artifact",
            CapacityPolicyMismatch::VersionMismatch,
        ),
    ] {
        let evidence =
            EquipmentCapacityEvidence::new(DefinitionVersion::new(version), artifact).unwrap();
        assert_eq!(evidence.definition_version().as_str(), version);
        assert_eq!(evidence.bucket_artifact_identity(), artifact);
        assert_eq!(policy().qualify(&evidence).unwrap_err(), reason);
    }
}

#[test]
fn nine_admits_ten_and_more_refuse_equipped_included() {
    let proof = prove(&acquisition(9)).unwrap();
    assert_eq!(proof.occupied(), 9);
    assert_eq!(proof.capacity(), 10);
    assert_eq!(proof.destination_bucket(), bucket());
    assert_eq!(proof.owner(), &owner());
    assert_eq!(proof.definition_version().as_str(), "v1");
    for occupied in [10, 11] {
        assert_eq!(
            prove(&acquisition(occupied)),
            Err(CapacityRefusal::AtCapacity {
                occupied,
                capacity: 10
            })
        );
    }
    assert_eq!(prove(&acquisition(0)).unwrap().occupied(), 0);
}

#[test]
fn missing_unsupported_unavailable_and_version_refusals_are_isolated() {
    let pair = acquisition(1);
    assert!(prove(&pair).is_ok());
    assert_eq!(
        qualified().prove(&pair, &owner(), None),
        Err(CapacityRefusal::MissingDestinationBucket)
    );
    assert_eq!(
        qualified().prove(&pair, &owner(), Some(BucketHash::new(0))),
        Err(CapacityRefusal::UnsupportedDestinationBucket)
    );
    let (input, rows) = pair.into_parts();
    assert_eq!(
        prove(&ProfileAcquisition::new(input.clone(), None).unwrap()),
        Err(CapacityRefusal::ObservationsUnavailable)
    );
    for version in ["v2", ""] {
        let mut input = input.clone();
        input.definition_version = DefinitionVersion::new(version);
        assert_eq!(
            prove(&ProfileAcquisition::new(input, rows.clone()).unwrap()),
            Err(CapacityRefusal::DefinitionVersionMismatch)
        );
    }
}

#[test]
fn only_destination_owner_character_actual_bucket_counts() {
    let (mut input, _) = acquisition(5).into_parts();
    input.items[1].location = ItemLocation::Character(CharacterId::new("other"));
    input.items[2].location = ItemLocation::Postmaster(owner());
    input.items[3].location = ItemLocation::Vault;
    let rows = input
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            InstancedBucketObservation::new(
                item.instance_id.clone(),
                if i == 4 {
                    BucketHash::new(BUCKETS[1])
                } else {
                    bucket()
                },
                item.location.clone(),
            )
            .unwrap()
        })
        .collect();
    let pair = ProfileAcquisition::new(input, Some(rows)).unwrap();
    assert_eq!(prove(&pair).unwrap().occupied(), 1);
}

#[test]
fn stacks_refuse_only_in_destination_character_bucket_never_quantity_as_slots() {
    let (input, rows) = acquisition(9).into_parts();
    for (container, hash, refuses) in [
        (StackContainer::CharacterInventory(owner()), bucket(), true),
        (
            StackContainer::CharacterInventory(owner()),
            BucketHash::new(BUCKETS[1]),
            false,
        ),
        (
            StackContainer::CharacterInventory(CharacterId::new("other")),
            bucket(),
            false,
        ),
        (StackContainer::Postmaster(owner()), bucket(), false),
        (StackContainer::ProfileInventory, bucket(), false),
    ] {
        let mut input = input.clone();
        input.stacks.push(
            StackObservation::new(ItemHash::new(42), hash, i64::from(i32::MAX), container).unwrap(),
        );
        let result = prove(&ProfileAcquisition::new(input, rows.clone()).unwrap());
        if refuses {
            assert_eq!(result, Err(CapacityRefusal::DestinationStackObserved));
        } else {
            assert_eq!(result.unwrap().occupied(), 9);
        }
    }
}
