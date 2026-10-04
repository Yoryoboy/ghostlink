use clap::Parser;
use destiny_app::{CharacterId, EquipRejection, MutationError, StepAction, StepOutcome};
use destiny_cli::equip_output::*;
use destiny_cli::{Cli, parse_process};

fn report(status: EquipStatus) -> EquipOutput {
    EquipOutput::from_shared(destiny_output::equip::EquipOutput {
        status,
        item: Some(EquipItem {
            name: "Ace of Spades".into(),
            instance_id: "123".into(),
            slot: Some("Kinetic".into()),
        }),
        character: Some(EquipCharacter {
            character_id: "456".into(),
            label: "Hunter".into(),
        }),
        steps: vec![],
        reason: None,
        profile_refresh: ProfileRefresh::Refreshed,
        observed: None,
    })
}

fn equip_action() -> StepAction {
    StepAction::Equip {
        character: CharacterId::new("456"),
    }
}

#[test]
fn equip_success_noop_json_and_exit_codes() {
    let mut output = report(EquipStatus::VerifiedSuccess);
    output.steps.push(EquipStep {
        ordinal: 1,
        kind: "transfer",
        from: Some(EquipLocation {
            kind: "vault",
            character_id: None,
            label: "Vault".into(),
        }),
        to: EquipLocation {
            kind: "character",
            character_id: Some("456".into()),
            label: "Hunter".into(),
        },
        outcome: "completed",
        reason: None,
    });
    output
        .steps
        .push(step_view(2, &equip_action(), StepOutcome::Completed, &[]));
    assert_eq!(
        output.render_text(),
        "Ace of Spades: Vault → Hunter (transferred) → equipped (Kinetic)\n✔ verified"
    );
    assert_eq!(output.exit_code(), 0);
    let value = output.json();
    assert_eq!(value["status"], "verified_success");
    assert_eq!(
        value["item"],
        serde_json::json!({"name":"Ace of Spades", "instance_id":"123", "slot":"Kinetic"})
    );
    assert_eq!(
        value["character"],
        serde_json::json!({"character_id":"456", "label":"Hunter"})
    );
    assert_eq!(
        value["steps"][1],
        serde_json::json!({"ordinal":2,"kind":"equip","to":{"kind":"character","character_id":"456","label":"456"},"outcome":"completed"})
    );
    assert!(value.get("plan_id").is_none());
    output.status = EquipStatus::VerifiedNoop;
    output.steps.clear();
    assert_eq!(
        output.render_text(),
        "Ace of Spades is already equipped on Hunter\n✔ verified"
    );
    assert_eq!(output.exit_code(), 0);
    assert_eq!(output.json()["status"], "verified_noop");
    for (status, code) in [
        (EquipStatus::Incomplete, 41),
        (EquipStatus::VerificationFailed, 42),
        (EquipStatus::Refused, 40),
    ] {
        output.status = status;
        assert_eq!(output.exit_code(), code);
    }
}

#[test]
fn equip_every_planning_refusal_has_a_human_reason() {
    for (error, expected) in [
        (
            MutationError::EquippedOnOtherCharacter,
            "equipped on another character",
        ),
        (MutationError::WrongClass, "class"),
        (MutationError::UnknownClass, "unknown"),
        (MutationError::NotEquippable, "equipment slot"),
        (MutationError::ExoticConflict, "another exotic"),
        (MutationError::UnknownRarity, "rarity is unknown"),
        (
            MutationError::UnknownNeighborSlot,
            "neighbor slot is unknown",
        ),
        (MutationError::MissingItem, "item was not found"),
        (MutationError::MissingDestination, "character was not found"),
        (
            MutationError::AmbiguousItem,
            "ambiguous; use the instance id",
        ),
        (
            MutationError::AmbiguousDestination,
            "ambiguous; use the character id",
        ),
    ] {
        let output = EquipOutput::refused(error);
        assert_eq!(output.json()["status"], "refused");
        assert!(
            output.render_text().contains(expected),
            "{}",
            output.render_text()
        );
        assert_ne!(output.exit_code(), 0);
    }
    assert_eq!(
        EquipOutput::refused(MutationError::AmbiguousItem).exit_code(),
        31
    );
}

#[test]
fn equip_every_provider_refusal_and_uncertainty_is_explained() {
    for (reason, expected) in [
        (
            EquipRejection::NotInSocialSpace,
            "social space, orbit, or offline",
        ),
        (EquipRejection::ExoticLimit, "exotic limit"),
        (EquipRejection::Unequippable, "not equippable"),
        (EquipRejection::ItemNotFound, "item was not found"),
        (EquipRejection::CharacterNotFound, "character was not found"),
        (EquipRejection::InvalidRequest, "request was rejected"),
    ] {
        let step = step_view(1, &equip_action(), StepOutcome::EquipRejected(reason), &[]);
        assert_eq!(step.outcome, "rejected");
        assert!(step.reason.as_ref().unwrap().contains(expected));
        let mut output = report(EquipStatus::Incomplete);
        output.steps.push(step);
        assert!(output.render_text().contains(expected));
        assert!(output.render_text().contains("profile refreshed"));
        assert_ne!(output.exit_code(), 0);
    }
    let mut output = report(EquipStatus::Incomplete);
    output.steps = vec![
        step_view(
            1,
            &StepAction::Transfer {
                from: destiny_app::ItemLocation::Vault,
                to: destiny_app::ItemLocation::Character(CharacterId::new("456")),
            },
            StepOutcome::Completed,
            &[],
        ),
        step_view(2, &equip_action(), StepOutcome::Uncertain, &[]),
        step_view(3, &equip_action(), StepOutcome::Unattempted, &[]),
    ];
    output.observed = Some(EquipObservation {
        location: EquipLocation {
            kind: "character",
            character_id: Some("456".into()),
            label: "Hunter".into(),
        },
        equipped: false,
    });
    let text = output.render_text();
    for expected in [
        "incomplete",
        "1. transfer: completed",
        "2. equip: uncertain",
        "3. equip: not run",
        "outcome is uncertain",
        "profile refreshed",
        "Hunter; not equipped",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert_eq!(output.json()["observed"]["equipped"], false);
    output.status = EquipStatus::VerificationFailed;
    output.profile_refresh = ProfileRefresh::Failed;
    assert!(output.render_text().contains("profile refresh failed"));
}

#[test]
fn equip_requires_item_and_explicit_character() {
    for args in [
        vec!["ghostlink", "equip"],
        vec!["ghostlink", "equip", "Ace"],
        vec!["ghostlink", "equip", "--character", "Hunter"],
    ] {
        let output = parse_process(args).unwrap_err();
        assert_eq!(output.exit_code, 2);
    }
    for selector in ["Hunter", "123"] {
        assert!(
            Cli::try_parse_from([
                "ghostlink",
                "equip",
                "Ace of Spades",
                "--character",
                selector,
                "--json"
            ])
            .is_ok()
        );
    }
}

#[test]
fn equip_process_streams_and_sanitization_follow_cli_conventions() {
    use destiny_cli::OutputMode;
    for status in [
        EquipStatus::VerifiedSuccess,
        EquipStatus::VerifiedNoop,
        EquipStatus::Incomplete,
        EquipStatus::VerificationFailed,
        EquipStatus::Refused,
    ] {
        let mut report = report(status);
        report.item.as_mut().unwrap().name = "Ace\n\u{1b}of Spades".into();
        for mode in [OutputMode::Human, OutputMode::Json] {
            let output = destiny_cli::runner::equip_process_output(mode, &report);
            assert_eq!(output.exit_code, report.exit_code());
            let body = if output.exit_code == 0 {
                assert!(output.stderr.is_empty());
                &output.stdout
            } else {
                assert!(output.stdout.is_empty());
                &output.stderr
            };
            if mode == OutputMode::Json {
                let value: serde_json::Value = serde_json::from_str(body).unwrap();
                assert_eq!(value, report.json());
            } else {
                assert!(!body.contains('\u{1b}'));
                assert!(!body.contains("Ace\n"));
            }
        }
    }
}
