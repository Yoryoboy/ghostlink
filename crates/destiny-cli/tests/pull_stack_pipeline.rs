//! Offline stack-pull command boundary tests: never credentials or live accounts.
use destiny_adapters::stack_bucket_capacity_policy::{
    PINNED_STACK_BUCKET_CAPACITY_VERSION, PINNED_STACK_BUCKET_PATH,
    pinned_stack_bucket_capacity_policy,
};
use destiny_app::test_support::*;
use destiny_app::*;
use destiny_cli::runner::{parse_process, run_pull_stack_with};

fn version() -> DefinitionVersion {
    DefinitionVersion::new(PINNED_STACK_BUCKET_CAPACITY_VERSION)
}
fn membership() -> DestinyMembership {
    DestinyMembership::new(MembershipType::new(3), MembershipId::new("123")).unwrap()
}
fn row(n: i64, container: StackContainer) -> StackObservation {
    StackObservation::new(
        ItemHash::new(7),
        BucketHash::new(if matches!(container, StackContainer::Postmaster(_)) {
            215593132
        } else {
            1469714392
        }),
        n,
        container,
    )
    .unwrap()
}
fn input(arrived: bool) -> InventorySnapshotInput {
    InventorySnapshotInput {
        membership_id: MembershipId::new("123"),
        membership_type: MembershipType::new(3),
        definition_version: version(),
        checked_at: Timestamp::new(100),
        source: SourceMarker::Profile,
        characters: vec![Character {
            id: CharacterId::new("2"),
            name: None,
            class: None,
            race: None,
            gender: None,
            light: None,
            date_last_played: None,
        }],
        items: vec![],
        stacks: vec![
            row(
                if arrived { 7 } else { 10 },
                StackContainer::Postmaster(CharacterId::new("2")),
            ),
            row(
                if arrived { 23 } else { 20 },
                StackContainer::ProfileInventory,
            ),
        ],
        plug_set_entries: vec![],
        profile_currencies_coverage: Default::default(),
    }
}
fn definition() -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(7),
        bucket_hash: Some(BucketHash::new(1469714392)),
        does_postmaster_pull_have_side_effects: Some(false),
        is_instance_item: Some(false),
        max_stack_size: Some(100),
        stack_unique_label: Some(String::new()),
        name: "PRIVATE_NAME\n\u{1b}".into(),
        item_type: None,
        tier: None,
        element: None,
        slot: None,
        class_type: None,
        plug_category: None,
        tags: vec![],
        version: version(),
    }
}
#[derive(Default)]
struct Delay(Vec<std::time::Duration>);
impl RetryDelay for Delay {
    fn wait(&mut self, value: std::time::Duration) {
        self.0.push(value);
    }
}
fn pipeline(
    d: Vec<ItemDefinition>,
    pre: InventorySnapshotInput,
    posts: Vec<Result<InventorySnapshotInput, AppError>>,
    receipt: Result<StackPullOutcome, AppError>,
    json: bool,
) -> (destiny_cli::ProcessOutput, QueuedStackPullGateway, Delay) {
    let mut profile = QueuedProfileGateway::new();
    for next in [Ok(pre.clone()), Ok(pre)].into_iter().chain(posts) {
        profile.queue_membership(Ok(membership()));
        profile.queue_acquisition(next.map(|i| ProfileAcquisition::new(i, None).unwrap()));
    }
    let mut manifest = QueuedManifestGateway::new();
    for _ in 0..2 {
        manifest.queue_capacity_evidence(Ok(EquipmentCapacityEvidence::new(
            version(),
            PINNED_STACK_BUCKET_PATH,
        )
        .unwrap()));
    }
    let mut service = StackPullService::new(
        ProfileSyncService::new(profile, InMemoryInventoryRepository::new()),
        InMemoryDefinitionRepository::new(version(), d),
        manifest,
        pinned_stack_bucket_capacity_policy(),
        FakeClock::fixed(Timestamp::new(100)),
        DeterministicPlanIds::starting_at(1),
    );
    let mut gateway = QueuedStackPullGateway::new();
    gateway.queue(receipt);
    let mut delay = Delay::default();
    let mut args = vec![
        "ghostlink",
        "pull-stack",
        "7",
        "--amount",
        "3",
        "--character",
        "2",
    ];
    if json {
        args.push("--json");
    }
    let output = run_pull_stack_with(args, |selection| {
        Ok(destiny_cli::pull_stack::pull_stack(
            selection,
            membership(),
            &mut service,
            &mut gateway,
            &mut delay,
            FreshnessPolicy { ttl_seconds: 300 },
        ))
    });
    (output, gateway, delay)
}
fn body(output: &destiny_cli::ProcessOutput) -> serde_json::Value {
    serde_json::from_str(if output.exit_code == 0 {
        &output.stdout
    } else {
        &output.stderr
    })
    .unwrap()
}
#[test]
fn verified_quantities_receipt_and_shared_text() {
    for receipt in [StackPullOutcome::Acknowledged, StackPullOutcome::Uncertain] {
        let (output, action, delay) = pipeline(
            vec![definition()],
            input(false),
            vec![Ok(input(true))],
            Ok(receipt),
            true,
        );
        assert_eq!(output.exit_code, 0);
        assert!(output.stderr.is_empty());
        let value = body(&output);
        assert_eq!(value["kind"], "stack_pull");
        assert_eq!(value["disposition"], "verified");
        assert_eq!(
            value["observations"][0]["classification"],
            "verified_arrival"
        );
        assert_eq!(
            value["observations"][0]["quantities"]["profile"]["post"],
            23
        );
        assert_eq!(action.calls(), 1);
        assert_eq!(action.requests()[0].quantity(), 3);
        assert_eq!(action.requests()[0].character().as_str(), "2");
        assert!(delay.0.is_empty());
    }
    let (text, _, _) = pipeline(
        vec![definition()],
        input(false),
        vec![Ok(input(true))],
        Ok(StackPullOutcome::Acknowledged),
        false,
    );
    assert_eq!(text.exit_code, 0);
    assert!(text.stdout.contains("stack_pull: verified"));
    assert!(text.stdout.contains("profile_quantity: 20 -> 23"));
    assert!(!text.stdout.contains("PRIVATE"));
}
#[test]
fn lag_and_observation_failure_are_uncertain_without_redispatch() {
    for posts in [
        vec![Ok(input(false)); 3],
        vec![Err(AppError::new(ErrorCategory::ProviderUnavailable)); 3],
    ] {
        let (output, action, delay) = pipeline(
            vec![definition()],
            input(false),
            posts,
            Ok(StackPullOutcome::Acknowledged),
            true,
        );
        assert_eq!(output.exit_code, 42);
        assert!(output.stdout.is_empty());
        assert_eq!(body(&output)["disposition"], "uncertain");
        assert_eq!(body(&output)["observations"].as_array().unwrap().len(), 3);
        assert_eq!(action.calls(), 1);
        assert_eq!(delay.0, vec![std::time::Duration::from_secs(2); 2]);
    }
}
#[test]
fn received_refusals_stay_refused_even_with_arrival() {
    for (receipt, exit) in [
        (StackPullOutcome::AuthenticationRefused, 11),
        (StackPullOutcome::Transient, 21),
        (
            StackPullOutcome::Rejected(StackPullRejection::InvalidRequest),
            40,
        ),
    ] {
        let (output, action, _) = pipeline(
            vec![definition()],
            input(false),
            vec![Ok(input(true))],
            Ok(receipt),
            true,
        );
        assert_eq!(output.exit_code, exit);
        assert_eq!(body(&output)["disposition"], "refused");
        assert_eq!(
            body(&output)["observations"][0]["classification"],
            "verified_arrival"
        );
        assert_eq!(action.calls(), 1);
    }
}
#[test]
fn planning_refusals_preserve_reasons_and_do_not_dispatch() {
    for case in 0..7 {
        let mut d = definition();
        let mut pre = input(false);
        let (defs, exit, reason) = match case {
            0 => (vec![], 33, "definition_missing"),
            1 => (vec![d.clone(), d], 31, "definition_ambiguous"),
            2 => {
                d.does_postmaster_pull_have_side_effects = Some(true);
                (vec![d], 40, "side_effects")
            }
            3 => {
                d.is_instance_item = Some(true);
                (vec![d], 40, "instanced")
            }
            4 => {
                d.max_stack_size = None;
                (vec![d], 40, "capacity")
            }
            5 => {
                pre.stacks.clear();
                (vec![d], 40, "capacity")
            }
            _ => {
                d.is_instance_item = None;
                (vec![d], 40, "instance_kind_unknown")
            }
        };
        let (output, action, delay) =
            pipeline(defs, pre, vec![], Ok(StackPullOutcome::Acknowledged), true);
        assert_eq!(output.exit_code, exit);
        let value = body(&output);
        assert_eq!(value["failure"]["reason"], reason);
        assert!(value["receipt"].is_null());
        assert_eq!(action.calls(), 0);
        assert!(delay.0.is_empty());
        assert!(!output.stderr.contains("PRIVATE"));
    }
}
#[test]
fn gateway_infrastructure_uses_category_exit_without_receipt() {
    let (output, action, _) = pipeline(
        vec![definition()],
        input(false),
        vec![],
        Err(AppError::new(ErrorCategory::ProviderUnavailable)),
        true,
    );
    assert_eq!(output.exit_code, 21);
    assert!(body(&output)["receipt"].is_null());
    assert_eq!(body(&output)["observations"], serde_json::json!([]));
    assert_eq!(action.calls(), 1);
}

#[test]
fn duplicate_stacks_use_smallest_quantity_not_sum() {
    let mut pre = input(false);
    pre.stacks
        .push(row(2, StackContainer::Postmaster(CharacterId::new("2"))));
    let (output, action, _) = pipeline(
        vec![definition()],
        pre,
        vec![],
        Ok(StackPullOutcome::Acknowledged),
        true,
    );
    assert_eq!(output.exit_code, 40);
    assert_eq!(
        body(&output)["failure"]["capacity_reason"]["kind"],
        "exceeds_smallest_postmaster_stack"
    );
    assert_eq!(action.calls(), 0);
}

#[test]
fn text_keeps_ordered_failures_and_null_quantities() {
    let (output, action, _) = pipeline(
        vec![definition()],
        input(false),
        vec![
            Ok(input(false)),
            Err(AppError::new(ErrorCategory::ProviderUnavailable)),
            Ok(input(false)),
        ],
        Ok(StackPullOutcome::Acknowledged),
        false,
    );
    assert_eq!(output.exit_code, 42);
    assert!(output.stdout.is_empty());
    let text = output.stderr;
    assert!(text.contains("classification: quantity_mismatch"));
    assert!(text.contains("classification: null"));
    assert!(text.contains("profile_quantity: 20 -> null"));
    assert!(text.contains("provider_unavailable"));
    assert!(text.find("observation 1:").unwrap() < text.find("observation 2:").unwrap());
    assert!(text.find("observation 2:").unwrap() < text.find("observation 3:").unwrap());
    assert!(!text.contains("PRIVATE"));
    assert_eq!(action.calls(), 1);
}

#[test]
fn invalid_arguments_never_construct_runtime() {
    for (slot, values) in [
        (
            2,
            vec![
                "-1",
                "4294967296",
                "01",
                "+1",
                " 1",
                "1 ",
                "1.0",
                "name",
                "",
                "\nPRIVATE",
            ],
        ),
        (
            4,
            vec![
                "0",
                "-1",
                "2147483648",
                "01",
                "+1",
                " 1",
                "1 ",
                "1.0",
                "",
                "\nPRIVATE",
            ],
        ),
        (
            6,
            vec![
                "0",
                "-1",
                "9223372036854775808",
                "01",
                "+1",
                "hunter",
                " 1",
                "1 ",
                "",
                "\nPRIVATE",
            ],
        ),
    ] {
        for value in values {
            let mut args = vec![
                "ghostlink",
                "pull-stack",
                "7",
                "--amount",
                "3",
                "--character",
                "2",
                "--json",
            ];
            args[slot] = value;
            let output = run_pull_stack_with(args, |_| panic!("no auth/service/action"));
            assert_eq!(output.exit_code, 2);
            assert!(output.stdout.is_empty());
            assert!(!output.stderr.contains("PRIVATE"));
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&output.stderr).unwrap()["error"]["code"],
                "invalid_request"
            );
        }
    }
    for args in [
        vec!["ghostlink", "pull-stack", "7"],
        vec!["ghostlink", "pull-stack", "7", "--amount", "3"],
        vec!["ghostlink", "pull-stack", "7", "--character", "2"],
        vec![
            "ghostlink",
            "pull-stack",
            "7",
            "--amount",
            "3",
            "--character",
            "2",
            "--vault",
        ],
        vec![
            "ghostlink",
            "pull-stack",
            "7",
            "--amount",
            "3",
            "--character",
            "2",
            "--amount",
            "4",
        ],
    ] {
        assert_eq!(
            run_pull_stack_with(args, |_| panic!("no runtime")).exit_code,
            2
        );
    }
}

#[test]
fn canonical_bounds_and_help() {
    for (hash, amount, owner) in [
        ("0", "1", "1"),
        ("4294967295", "2147483647", "9223372036854775807"),
    ] {
        let mut called = false;
        let output = run_pull_stack_with(
            [
                "ghostlink",
                "pull-stack",
                hash,
                "--amount",
                amount,
                "--character",
                owner,
            ],
            |selection| {
                called = true;
                assert_eq!(selection.item_hash.value().to_string(), hash);
                assert_eq!(selection.amount.to_string(), amount);
                assert_eq!(selection.owner.as_str(), owner);
                Err(destiny_cli::CliError::invalid_request())
            },
        );
        assert!(called);
        assert_eq!(output.exit_code, 2);
    }
    let help = parse_process(["ghostlink", "pull-stack", "--help"]).unwrap_err();
    assert_eq!(help.exit_code, 0);
    for text in [
        "ITEM_HASH",
        "--amount",
        "--character",
        "canonical",
        "verified",
    ] {
        assert!(help.stdout.contains(text), "{text}: {}", help.stdout);
    }
}
