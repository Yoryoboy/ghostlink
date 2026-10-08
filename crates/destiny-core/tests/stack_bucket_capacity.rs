use destiny_core::*;

const CONSUMABLES: i64 = 1469714392;
const MODIFICATIONS: i64 = 3313201758;

fn policy() -> StackBucketCapacityPolicy {
    StackBucketCapacityPolicy::new(
        DefinitionVersion::new("v1"),
        "buckets.json",
        vec![
            (BucketHash::new(CONSUMABLES), 50, true),
            (BucketHash::new(MODIFICATIONS), 50, true),
        ],
    )
    .unwrap()
}

fn qualified() -> QualifiedStackBucketCapacityPolicy {
    policy()
        .qualify(
            &StackBucketCapacityEvidence::new(DefinitionVersion::new("v1"), "buckets.json")
                .unwrap(),
        )
        .unwrap()
}

fn row(hash: i64, bucket: i64, quantity: i64, container: StackContainer) -> StackObservation {
    StackObservation::new(
        ItemHash::new(hash),
        BucketHash::new(bucket),
        quantity,
        container,
    )
    .unwrap()
}

#[test]
fn profile_bucket_rows_preserve_multiplicity_and_exclude_all_other_sources() {
    for bucket in [CONSUMABLES, MODIFICATIONS] {
        let mut rows = vec![
            row(42, bucket, 3, StackContainer::ProfileInventory),
            row(42, bucket, 3, StackContainer::ProfileInventory),
            row(99, bucket, 100, StackContainer::ProfileInventory),
            row(42, 138197802, 100, StackContainer::ProfileInventory),
            row(42, bucket, 100, StackContainer::ProfileCurrencies),
        ];
        for owner in ["2", "3"] {
            rows.push(row(
                42,
                bucket,
                100,
                StackContainer::Postmaster(CharacterId::new(owner)),
            ));
            rows.push(row(
                42,
                bucket,
                100,
                StackContainer::CharacterInventory(CharacterId::new(owner)),
            ));
        }
        let facts = qualified()
            .derive(BucketHash::new(bucket), ItemHash::new(42), &rows)
            .unwrap();
        assert_eq!(facts.capacity, Some(50));
        assert_eq!(facts.occupied, Some(3));
        assert_eq!(facts.existing_quantity, Some(6));
        assert_eq!(facts.account_wide, Some(true));
        assert_eq!(facts.max_stack_size, None);
        assert_eq!(facts.unique_stack, None);
        assert_eq!(facts.side_effects, None);
        rows.reverse();
        assert_eq!(
            qualified()
                .derive(BucketHash::new(bucket), ItemHash::new(42), &rows)
                .unwrap(),
            facts
        );
    }
}

#[test]
fn empty_profile_is_known_zero_and_quantity_is_not_narrowed() {
    let bucket = BucketHash::new(CONSUMABLES);
    let empty = qualified().derive(bucket, ItemHash::new(42), &[]).unwrap();
    assert_eq!(empty.occupied, Some(0));
    assert_eq!(empty.existing_quantity, Some(0));
    let rows = vec![
        row(
            42,
            CONSUMABLES,
            i64::from(i32::MAX),
            StackContainer::ProfileInventory
        );
        3
    ];
    let facts = qualified()
        .derive(bucket, ItemHash::new(42), &rows)
        .unwrap();
    assert_eq!(facts.occupied, Some(3));
    assert_eq!(facts.existing_quantity, Some(3 * i32::MAX as u64));
    let other = qualified()
        .derive(bucket, ItemHash::new(99), &rows)
        .unwrap();
    assert_eq!(other.occupied, Some(3));
    assert_eq!(other.existing_quantity, Some(0));
}

#[test]
fn unknown_bucket_and_identity_drift_refuse() {
    for hash in [0, 375726501, 3865314626, 1498876634] {
        assert_eq!(
            qualified().derive(BucketHash::new(hash), ItemHash::new(42), &[]),
            Err(StackPullRefusal::UnknownCapacity)
        );
    }
    for (version, artifact, reason) in [
        (
            "v2",
            "buckets.json",
            CapacityPolicyMismatch::VersionMismatch,
        ),
        ("v1", "new.json", CapacityPolicyMismatch::ArtifactMismatch),
        ("v2", "new.json", CapacityPolicyMismatch::VersionMismatch),
    ] {
        let evidence =
            StackBucketCapacityEvidence::new(DefinitionVersion::new(version), artifact).unwrap();
        assert_eq!(policy().qualify(&evidence).unwrap_err(), reason);
    }
}

#[test]
fn malformed_or_unsupported_policy_tables_refuse() {
    let valid = vec![
        (BucketHash::new(CONSUMABLES), 50, true),
        (BucketHash::new(MODIFICATIONS), 50, true),
    ];
    let mut cases = vec![vec![], vec![valid[0]], vec![valid[0], valid[0]]];
    for entry in [
        (BucketHash::new(3865314626), 50, true),
        (BucketHash::new(CONSUMABLES), 0, true),
        (BucketHash::new(CONSUMABLES), 50, false),
    ] {
        cases.push(vec![entry, valid[1]]);
    }
    for table in cases {
        assert!(
            StackBucketCapacityPolicy::new(DefinitionVersion::new("v1"), "buckets.json", table)
                .is_err()
        );
    }
    for (version, artifact) in [("", "buckets.json"), ("v1", "")] {
        assert!(
            StackBucketCapacityPolicy::new(
                DefinitionVersion::new(version),
                artifact,
                valid.clone()
            )
            .is_err()
        );
    }
}

#[test]
fn derived_account_facts_use_normal_capacity_formulas_for_any_postmaster_owner() {
    let rows = vec![row(42, CONSUMABLES, 7, StackContainer::ProfileInventory); 50];
    let facts = qualified()
        .derive(BucketHash::new(CONSUMABLES), ItemHash::new(42), &rows)
        .unwrap();
    for owner in ["2", "3"] {
        let membership =
            DestinyMembership::new(MembershipType::new(3), MembershipId::new("123")).unwrap();
        let request =
            StackPullRequest::new(membership, ItemHash::new(42), CharacterId::new(owner), 1)
                .unwrap();
        let non_unique = StackPullCapacityInput {
            max_stack_size: Some(99),
            unique_stack: Some(false),
            side_effects: Some(false),
            ..facts
        };
        assert_eq!(
            evaluate_stack_pull_capacity(&request, &non_unique),
            StackPullDecision::Eligible
        );
        let unique = StackPullCapacityInput {
            max_stack_size: Some(351),
            unique_stack: Some(true),
            ..non_unique
        };
        assert_eq!(
            evaluate_stack_pull_capacity(&request, &unique),
            StackPullDecision::Eligible
        );
        let full = StackPullCapacityInput {
            max_stack_size: Some(350),
            ..unique
        };
        assert_eq!(
            evaluate_stack_pull_capacity(&request, &full),
            StackPullDecision::Refused(StackPullRefusal::InsufficientCapacity)
        );
    }
}
