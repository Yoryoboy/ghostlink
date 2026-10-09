use clap::Parser;
use destiny_app::*;
use destiny_cli::command::{Cli, Command};
use destiny_cli::pull_output::PullOutput;
use destiny_cli::pull_stack_output::StackPullOutput;

#[test]
fn pull_refusals_are_sentences_not_json() {
    for refusal in [
        InstancedPullRefusal::Capacity(CapacityRefusal::AtCapacity {
            occupied: 10,
            capacity: 10,
        }),
        InstancedPullRefusal::SideEffects,
        InstancedPullRefusal::SideEffectsUnknown,
        InstancedPullRefusal::DefinitionVersionMismatch,
    ] {
        let text = PullOutput::planning_failed(&InstancedPullPlanningError::Refused(refusal))
            .render_text();
        assert!(!text.contains('{') && !text.contains("null"), "{text}");
        assert!(text.contains(';') || text.contains('.'), "{text}");
    }
    for refusal in [
        StackPullPlanningRefusal::Capacity(StackPullRefusal::InsufficientCapacity),
        StackPullPlanningRefusal::Capacity(StackPullRefusal::UnknownUniqueness),
        StackPullPlanningRefusal::SideEffects,
        StackPullPlanningRefusal::SideEffectsUnknown,
        StackPullPlanningRefusal::DefinitionVersionMismatch,
    ] {
        let text = StackPullOutput::planning_failed(&StackPullPlanningError::Refused(refusal))
            .render_text();
        assert!(!text.contains('{') && !text.contains("null"), "{text}");
        assert!(text.contains(';') || text.contains('.'), "{text}");
    }
}

#[test]
fn mismatch_names_versions_without_changing_json() {
    let versions = Some((
        DefinitionVersion::new("snapshot-v2"),
        DefinitionVersion::new("stored-v1"),
    ));
    let pull = PullOutput::planning_failed(&InstancedPullPlanningError::Refused(
        InstancedPullRefusal::DefinitionVersionMismatch,
    ));
    let before = pull.json();
    let pull = pull.with_versions(versions.clone());
    assert_eq!(before, pull.json());
    let stack = StackPullOutput::planning_failed(&StackPullPlanningError::Refused(
        StackPullPlanningRefusal::DefinitionVersionMismatch,
    ))
    .with_versions(versions);
    for text in [pull.render_text(), stack.render_text()] {
        for expected in ["snapshot-v2", "stored-v1", "ghostlink sync"] {
            assert!(text.contains(expected), "{text}");
        }
    }
}

#[test]
fn destination_full_receipt_is_actionable() {
    let value = serde_json::json!({"disposition":"refused", "receipt":{"kind":"rejected", "reason":"destination_full"}});
    let text = destiny_output::instanced_pull::render_pull_text(&value);
    assert!(text.contains("Destination is full; free a slot and retry."));
    assert!(!text.contains('{'));
}

#[test]
fn explicit_selectors_and_hash_alias_parse() {
    for (flag, value) in [("--slot", "2"), ("--name", "PvP")] {
        let cli = Cli::try_parse_from([
            "ghostlink",
            "apply-loadout",
            flag,
            value,
            "--character",
            "hunter",
        ])
        .unwrap();
        let Command::ApplyLoadout(args) = cli.command else {
            panic!()
        };
        assert!(args.request().is_ok());
        assert!(
            Cli::try_parse_from([
                "ghostlink",
                "apply-loadout",
                "1",
                flag,
                value,
                "--character",
                "hunter"
            ])
            .is_err()
        );
    }
    assert!(
        Cli::try_parse_from([
            "ghostlink",
            "apply-loadout",
            "--slot",
            "2",
            "--name",
            "PvP",
            "--character",
            "hunter"
        ])
        .is_err()
    );
    let cli = Cli::try_parse_from([
        "ghostlink",
        "pull-stack",
        "--item-hash",
        "123",
        "--amount",
        "1",
        "--character",
        "hunter",
    ])
    .unwrap();
    let Command::PullStack(args) = cli.command else {
        panic!()
    };
    assert_eq!(args.selection().unwrap().owner.as_str(), "hunter");
}
