use destiny_core::*;

fn membership(kind: u32) -> DestinyMembership {
    DestinyMembership::new(MembershipType::new(kind), MembershipId::new("00123456789")).unwrap()
}
fn wire(owner: &str, hash: i64, kind: u32, amount: i64) -> Result<StackPullRequest, AppError> {
    StackPullRequest::new(
        membership(kind),
        ItemHash::new(hash),
        CharacterId::new(owner),
        amount,
    )
}
fn request(amount: i64) -> StackPullRequest {
    wire("2", 42, 3, amount).unwrap()
}
fn capacity() -> StackPullCapacityInput {
    StackPullCapacityInput {
        capacity: Some(10),
        occupied: Some(9),
        existing_quantity: Some(0),
        max_stack_size: Some(10),
        unique_stack: Some(false),
        account_wide: Some(false),
        side_effects: Some(false),
    }
}
fn stack(hash: i64, quantity: i64, container: StackContainer) -> StackObservation {
    StackObservation::new(
        ItemHash::new(hash),
        BucketHash::new(215593132),
        quantity,
        container,
    )
    .unwrap()
}
fn rows(quantities: &[i64]) -> Vec<StackObservation> {
    quantities
        .iter()
        .map(|q| stack(42, *q, StackContainer::Postmaster(CharacterId::new("2"))))
        .collect()
}
fn refused(reason: StackPullRefusal) -> StackPullDecision {
    StackPullDecision::Refused(reason)
}
fn check_capacity(input: &StackPullCapacityInput, amount: i64, expected: StackPullDecision) {
    assert_eq!(
        evaluate_stack_pull_capacity(&request(amount), input),
        expected
    );
}

#[test]
fn wire_boundaries_accessors_and_redaction() {
    for amount in [1, i64::from(i32::MAX)] {
        for hash in [0, i64::from(u32::MAX)] {
            for kind in [0, 3, 254, i32::MAX as u32] {
                for owner in ["1", "9223372036854775807"] {
                    let pull = wire(owner, hash, kind, amount).unwrap();
                    assert_eq!(pull.membership(), &membership(kind));
                    assert_eq!(pull.item_hash(), ItemHash::new(hash));
                    assert_eq!(pull.character().as_str(), owner);
                    assert_eq!(pull.quantity(), amount as i32);
                    assert_eq!(pull.item_id(), "0");
                    assert_eq!(StackPullRequest::ITEM_ID, "0");
                    assert_eq!(pull.clone(), pull);
                    let debug = format!("{pull:?}");
                    assert!(!debug.contains("00123456789"));
                    assert!(!debug.contains("9223372036854775807"));
                }
            }
        }
    }
}

#[test]
fn wire_invalid_values_are_sanitized() {
    let assert_invalid = |owner: &str, hash: i64, kind: u32, amount: i64| {
        let error = wire(owner, hash, kind, amount).unwrap_err();
        assert_eq!(error, AppError::new(ErrorCategory::ProviderValidation));
        assert_eq!(error.to_string(), "the provider rejected the request");
    };
    for amount in [i64::MIN, -1, 0, i64::from(i32::MAX) + 1, i64::MAX] {
        assert_invalid("2", 42, 3, amount);
    }
    for hash in [i64::MIN, -1, i64::from(u32::MAX) + 1, i64::MAX] {
        assert_invalid("2", hash, 3, 1);
    }
    for kind in [i32::MAX as u32 + 1, u32::MAX] {
        assert_invalid("2", 42, kind, 1);
    }
    let oversized = "1".repeat(10_000);
    for owner in [
        "",
        "0",
        "00",
        "01",
        "-1",
        "+1",
        " 1",
        "1 ",
        "1\n",
        "\t1",
        "1\0",
        "１",
        "١",
        "1.0",
        "1e1",
        "9223372036854775808",
        "9999999999999999999",
        "10000000000000000000",
        oversized.as_str(),
    ] {
        assert_invalid(owner, 42, 3, 1);
    }
}

#[test]
fn selection_uses_smallest_without_merging_and_filters_context() {
    assert_eq!(
        evaluate_stack_pull(&request(1), &[], &capacity()),
        refused(StackPullRefusal::NoPostmasterStack)
    );
    let mut observations = rows(&[10, 3, 3]);
    observations.extend([
        stack(42, 1, StackContainer::Postmaster(CharacterId::new("3"))),
        stack(43, 1, StackContainer::Postmaster(CharacterId::new("2"))),
        stack(
            42,
            1,
            StackContainer::CharacterInventory(CharacterId::new("2")),
        ),
        stack(42, 1, StackContainer::ProfileInventory),
        stack(42, 0, StackContainer::ProfileCurrencies),
    ]);
    for amount in [1, 3] {
        assert_eq!(
            evaluate_stack_pull(&request(amount), &observations, &capacity()),
            StackPullDecision::Eligible
        );
    }
    for amount in [4, 6, 10] {
        assert_eq!(
            evaluate_stack_pull(&request(amount), &observations, &capacity()),
            refused(StackPullRefusal::ExceedsSmallestPostmasterStack)
        );
    }
    observations.reverse();
    assert_eq!(
        evaluate_stack_pull(&request(4), &observations, &capacity()),
        refused(StackPullRefusal::ExceedsSmallestPostmasterStack)
    );
    assert_eq!(
        evaluate_stack_pull(&request(1), &observations[0..1], &capacity()),
        refused(StackPullRefusal::NoPostmasterStack)
    );
}

#[test]
fn non_unique_formula_exhaustive_small_domain() {
    for max in 1..=6 {
        for slots in 0..=4 {
            for occupied in 0..=5 {
                for existing in 0..=18 {
                    let input = StackPullCapacityInput {
                        capacity: Some(slots),
                        occupied: Some(occupied),
                        existing_quantity: Some(existing),
                        max_stack_size: Some(max),
                        ..capacity()
                    };
                    let open = slots.saturating_sub(occupied);
                    let remainder = existing % max as u64;
                    let room = open * max as u64
                        + if remainder == 0 {
                            0
                        } else {
                            max as u64 - remainder
                        };
                    for amount in 1..=30 {
                        let expected = if amount as u64 <= room {
                            StackPullDecision::Eligible
                        } else {
                            refused(StackPullRefusal::InsufficientCapacity)
                        };
                        assert_eq!(
                            evaluate_stack_pull_capacity(&request(amount), &input),
                            expected,
                            "max={max} slots={slots} occupied={occupied} existing={existing} amount={amount}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn unique_stack_needs_one_slot_or_only_tops_off() {
    for open in [0, 1, 2] {
        for existing in [0, 1, 9, 10, 11] {
            let input = StackPullCapacityInput {
                capacity: Some(open),
                occupied: Some(0),
                existing_quantity: Some(existing),
                unique_stack: Some(true),
                ..capacity()
            };
            for amount in [1, 9, 10, 11] {
                let fits = if existing == 0 {
                    open >= 1 && amount <= 10
                } else {
                    existing <= 10 && amount as u64 <= 10 - existing
                };
                let expected = if existing > 10 {
                    refused(StackPullRefusal::InvalidCapacityMetadata)
                } else if fits {
                    StackPullDecision::Eligible
                } else {
                    refused(StackPullRefusal::InsufficientCapacity)
                };
                check_capacity(&input, amount, expected);
            }
        }
    }
}

#[test]
fn metadata_unknowns_and_safety_refusals_are_distinct() {
    // Each refusal has a fully known, eligible adjacent control.
    check_capacity(&capacity(), 1, StackPullDecision::Eligible);
    macro_rules! check_field {
        ($field:ident, $value:expr, $reason:ident) => {{
            let input = StackPullCapacityInput {
                $field: $value,
                ..capacity()
            };
            let expected = refused(StackPullRefusal::$reason);
            check_capacity(&input, 1, expected);
            assert_eq!(
                evaluate_stack_pull(&request(1), &rows(&[1]), &input),
                expected
            );
        }};
    }
    check_field!(capacity, None, UnknownCapacity);
    check_field!(occupied, None, UnknownOccupancy);
    check_field!(existing_quantity, None, UnknownExistingQuantity);
    check_field!(max_stack_size, None, UnknownMaxStackSize);
    check_field!(unique_stack, None, UnknownUniqueness);
    check_field!(account_wide, None, UnknownAccountWide);
    check_field!(side_effects, None, UnknownSideEffects);
    check_field!(account_wide, Some(true), AccountWideDestination);
    check_field!(side_effects, Some(true), PullHasSideEffects);
    check_field!(max_stack_size, Some(0), InvalidCapacityMetadata);
    check_field!(max_stack_size, Some(-1), InvalidCapacityMetadata);
}

#[test]
fn checked_arithmetic_and_large_wire_boundary() {
    let max = i64::from(i32::MAX);
    let input = StackPullCapacityInput {
        max_stack_size: Some(i32::MAX),
        ..capacity()
    };
    assert_eq!(
        evaluate_stack_pull(&request(max), &rows(&[max]), &input),
        StackPullDecision::Eligible
    );
    let input = StackPullCapacityInput {
        capacity: Some(u64::MAX),
        occupied: Some(0),
        max_stack_size: Some(2),
        ..capacity()
    };
    check_capacity(&input, 1, refused(StackPullRefusal::ArithmeticOverflow));
    let input = StackPullCapacityInput {
        capacity: Some(u64::MAX / 3),
        occupied: Some(0),
        existing_quantity: Some(1),
        max_stack_size: Some(3),
        ..capacity()
    };
    check_capacity(&input, 1, refused(StackPullRefusal::ArithmeticOverflow));
    let input = StackPullCapacityInput {
        existing_quantity: Some(0),
        ..input
    };
    check_capacity(&input, 1, StackPullDecision::Eligible);
    let input = StackPullCapacityInput {
        existing_quantity: Some(u64::MAX),
        capacity: Some(0),
        occupied: Some(0),
        max_stack_size: Some(10),
        ..capacity()
    };
    check_capacity(&input, 5, StackPullDecision::Eligible);
    check_capacity(&input, 6, refused(StackPullRefusal::InsufficientCapacity));
}

#[test]
fn outcomes_and_one_shot_gateway_preserve_uncertainty() {
    struct Gateway(Vec<StackPullRequest>, StackPullOutcome);
    impl StackPullGateway for Gateway {
        fn pull(&mut self, request: &StackPullRequest) -> Result<StackPullOutcome, AppError> {
            self.0.push(request.clone());
            Ok(self.1)
        }
    }
    let outcomes = [
        StackPullOutcome::Acknowledged,
        StackPullOutcome::Rejected(StackPullRejection::InvalidRequest),
        StackPullOutcome::AuthenticationRefused,
        StackPullOutcome::Transient,
        StackPullOutcome::Uncertain,
    ];
    for (index, outcome) in outcomes.iter().enumerate() {
        for other in &outcomes[index + 1..] {
            assert_ne!(outcome, other);
        }
        let mut gateway = Gateway(vec![], *outcome);
        let port: &mut dyn StackPullGateway = &mut gateway;
        let pull = request(3);
        assert_eq!(port.pull(&pull), Ok(*outcome));
        assert_eq!(gateway.0, vec![pull]);
    }
    assert_eq!(
        StackPullRejection::InvalidRequest.category(),
        ErrorCategory::ProviderValidation
    );
}
