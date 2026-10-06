use destiny_core::{
    BucketHash, CharacterId, ErrorCategory, ItemHash, StackContainer, StackObservation,
};

#[test]
fn observation_retains_definition_runtime_bucket_quantity_and_owner() {
    let owner = CharacterId::new("character");
    let container = StackContainer::Postmaster(owner.clone());
    let stack = StackObservation::new(ItemHash::new(42), BucketHash::new(7), 3, container)
        .expect("valid stack observation");

    assert_eq!(stack.item_hash(), ItemHash::new(42));
    assert_eq!(stack.bucket_hash(), BucketHash::new(7));
    assert_eq!(stack.quantity(), 3_i32);
    assert_eq!(stack.container(), &StackContainer::Postmaster(owner));
}

fn stack(container: StackContainer, quantity: i64) -> StackObservation {
    StackObservation::new(ItemHash::new(42), BucketHash::new(7), quantity, container)
        .expect("valid stack observation")
}

fn normalized(mut rows: Vec<StackObservation>) -> Vec<StackObservation> {
    rows.sort_unstable();
    rows
}

#[test]
fn quantities_accept_positive_int32_boundaries() {
    for quantity in [1, i64::from(i32::MAX)] {
        let row = stack(StackContainer::ProfileInventory, quantity);
        assert_eq!(i64::from(row.quantity()), quantity);
    }
}

#[test]
fn quantities_reject_zero_negative_and_overflow_values() {
    for quantity in [i64::MIN, -1, 0, i64::from(i32::MAX) + 1, i64::MAX] {
        let error = StackObservation::new(
            ItemHash::new(42),
            BucketHash::new(7),
            quantity,
            StackContainer::ProfileInventory,
        )
        .expect_err("invalid quantity must not be narrowed or accepted");
        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
        assert_eq!(error.to_string(), "the provider rejected the request");
    }
}

#[test]
fn hashes_accept_the_entire_uint32_range_including_zero() {
    for item in [0, 1, i64::from(u32::MAX)] {
        for bucket in [0, 1, i64::from(u32::MAX)] {
            let row = StackObservation::new(
                ItemHash::new(item),
                BucketHash::new(bucket),
                1,
                StackContainer::ProfileInventory,
            )
            .expect("valid unsigned hash boundaries");
            assert_eq!(row.item_hash().value(), item);
            assert_eq!(row.bucket_hash().value(), bucket);
        }
    }
}

#[test]
fn each_hash_rejects_negative_and_out_of_range_values() {
    for invalid in [i64::MIN, -1, i64::from(u32::MAX) + 1, i64::MAX] {
        for (item, bucket) in [(invalid, 7), (42, invalid)] {
            let error = StackObservation::new(
                ItemHash::new(item),
                BucketHash::new(bucket),
                1,
                StackContainer::ProfileInventory,
            )
            .expect_err("invalid hash must not be accepted");
            assert_eq!(error.category(), ErrorCategory::ProviderValidation);
        }
    }
}

#[test]
fn scopes_and_character_owners_remain_distinct() {
    let first = CharacterId::new("first");
    let second = CharacterId::new("second");
    let scopes = [
        StackContainer::ProfileInventory,
        StackContainer::ProfileCurrencies,
        StackContainer::CharacterInventory(first.clone()),
        StackContainer::CharacterInventory(second.clone()),
        StackContainer::Postmaster(first),
        StackContainer::Postmaster(second),
    ];
    for (index, scope) in scopes.iter().enumerate() {
        let row = stack(scope.clone(), 1);
        assert_eq!(row.container(), scope);
        for other in &scopes[index + 1..] {
            assert_ne!(row, stack(other.clone(), 1));
        }
    }
}

#[test]
fn normalization_is_permutation_independent_and_preserves_duplicate_rows() {
    let material = stack(StackContainer::ProfileInventory, 5);
    let currency = stack(StackContainer::ProfileCurrencies, 5);
    let postmaster = stack(StackContainer::Postmaster(CharacterId::new("owner")), 2);
    let expected = normalized(vec![
        material.clone(),
        material.clone(),
        currency.clone(),
        postmaster.clone(),
    ]);
    let permuted = normalized(vec![
        postmaster,
        material.clone(),
        currency,
        material.clone(),
    ]);
    assert_eq!(expected, permuted);
    assert_eq!(expected.len(), 4);
    assert_eq!(expected.iter().filter(|row| **row == material).count(), 2);
    assert_eq!(normalized(expected.clone()), expected);
    assert!(normalized(Vec::new()).is_empty());
}

#[test]
fn equality_detects_quantity_and_multiplicity_not_only_aggregate_totals() {
    let one = stack(StackContainer::ProfileInventory, 1);
    let two = stack(StackContainer::ProfileInventory, 2);
    assert_ne!(normalized(vec![one.clone()]), normalized(vec![two.clone()]));
    assert_ne!(
        normalized(vec![one.clone()]),
        normalized(vec![one.clone(), one.clone()]),
    );
    assert_ne!(normalized(vec![one.clone(), one]), normalized(vec![two]));
}

#[test]
fn equality_includes_item_definition_and_actual_runtime_bucket() {
    let original = stack(StackContainer::ProfileInventory, 1);
    for (item, bucket) in [(43, 7), (42, 8)] {
        let different = StackObservation::new(
            ItemHash::new(item),
            BucketHash::new(bucket),
            1,
            StackContainer::ProfileInventory,
        )
        .expect("different valid observation");
        assert_ne!(
            normalized(vec![original.clone()]),
            normalized(vec![different])
        );
    }
}

#[test]
fn identical_rows_have_no_synthetic_identity_and_owner_debug_remains_redacted() {
    let first = stack(
        StackContainer::Postmaster(CharacterId::new("owner-sentinel")),
        3,
    );
    let second = stack(
        StackContainer::Postmaster(CharacterId::new("owner-sentinel")),
        3,
    );
    assert_eq!(first, second);
    let debug = format!("{first:?}");
    assert!(!debug.contains("owner-sentinel"));
    assert!(!debug.contains("instance_id"));
    assert!(!debug.contains("stack_id"));
}
