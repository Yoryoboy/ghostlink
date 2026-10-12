use clap::Parser;
use destiny_cli::command::Cli;

#[test]
fn build_commands_parse_all_sources_and_selectors() {
    for args in [
        vec!["ghostlink", "build", "list"],
        vec![
            "ghostlink",
            "build",
            "check",
            "id",
            "--character",
            "hunter",
            "--json",
        ],
        vec!["ghostlink", "build", "check", "--name", "Raid"],
        vec!["ghostlink", "build", "apply", "id", "--dry-run"],
        vec![
            "ghostlink",
            "build",
            "apply",
            "--name",
            "Raid",
            "--character",
            "hunter",
            "--dry-run",
            "--json",
        ],
        vec!["ghostlink", "build", "import", "-"],
        vec!["ghostlink", "build", "show", "--name", "Raid"],
        vec!["ghostlink", "build", "export", "id"],
        vec!["ghostlink", "build", "delete", "id"],
        vec![
            "ghostlink",
            "build",
            "from-equipped",
            "Raid",
            "--character",
            "hunter",
        ],
        vec![
            "ghostlink",
            "build",
            "from-loadout",
            "--slot",
            "1",
            "--character",
            "hunter",
        ],
    ] {
        assert!(Cli::try_parse_from(args.clone()).is_ok(), "{args:?}");
    }
    assert!(Cli::try_parse_from(["ghostlink", "build", "show", "id", "--name", "Raid"]).is_err());
}

#[test]
fn share_import_handler_returns_typed_error_before_configuration() {
    for input in ["https://dim.gg/abcdefg", "dim.gg/abcdefg", "abcdefg"] {
        let args = destiny_cli::command::BuildArgs {
            command: destiny_cli::command::BuildCommand::Import {
                input: input.into(),
            },
        };
        let output = destiny_cli::builds::compose(&args).unwrap();
        assert_eq!(output.exit_code, 40);
        assert_eq!(
            output.value["error"]["reason"],
            "share_link_requires_online_import"
        );
    }
}

#[test]
fn stdin_input_is_bounded_utf8() {
    assert_eq!(destiny_cli::builds::read_input(&b"{}"[..]).unwrap(), "{}");
    assert!(destiny_cli::builds::read_input(&b"\xff"[..]).is_err());
    let oversized = vec![b' '; destiny_adapters::dim_loadout::MAX_DIM_INPUT_BYTES + 1];
    assert!(destiny_cli::builds::read_input(oversized.as_slice()).is_err());
}
