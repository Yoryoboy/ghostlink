use clap::Parser;
use destiny_app::MutationError;
use destiny_cli::command::{Cli, Command};
use destiny_cli::lock_output::LockOutput;

#[test]
fn lock_commands_parse_explicit_state_and_validate_before_io() {
    for command in ["lock", "unlock"] {
        let cli = Cli::try_parse_from(["ghostlink", command, "Ace of Spades", "--json"]).unwrap();
        assert!(cli.json);
        match cli.command {
            Command::Lock(args) | Command::Unlock(args) => args.validate().unwrap(),
            _ => panic!("lock command"),
        }
        assert!(Cli::try_parse_from(["ghostlink", command]).is_err());
        assert!(Cli::try_parse_from(["ghostlink", command, "123", "--character", "456"]).is_err());
        match Cli::try_parse_from(["ghostlink", command, "bad\nitem"])
            .unwrap()
            .command
        {
            Command::Lock(args) | Command::Unlock(args) => assert!(args.validate().is_err()),
            _ => panic!("lock command"),
        }
    }
}

#[test]
fn lock_refusal_exit_policy_is_category_based() {
    for (error, code) in [
        (MutationError::MissingItem, 32),
        (MutationError::AmbiguousItem, 31),
        (MutationError::StaleRevision, 30),
    ] {
        let output = LockOutput::refused(error, true);
        assert_eq!(output.exit_code(), code);
        assert_eq!(output.json()["status"], "refused");
        assert!(!output.render_text().contains("verified"));
    }
}
