use clap::Parser;
use destiny_cli::command::{Cli, Command};

#[test]
fn apply_loadout_requires_character_and_accepts_slot_or_name() {
    for selector in ["3", "PvE"] {
        let cli = Cli::try_parse_from([
            "ghostlink",
            "apply-loadout",
            selector,
            "--character",
            "hunter",
            "--json",
        ])
        .expect("apply-loadout command exists");
        assert!(matches!(cli.command, Command::ApplyLoadout(_)));
        assert!(cli.json);
    }
    assert!(Cli::try_parse_from(["ghostlink", "apply-loadout", "3"]).is_err());
}
