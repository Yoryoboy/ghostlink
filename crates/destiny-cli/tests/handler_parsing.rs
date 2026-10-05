use clap::Parser;
use destiny_cli::{CallbackMode, Cli, Command};

#[test]
fn callback_defaults_to_auto_and_overrides_still_parse() {
    for (value, expected) in [
        (None, "auto"),
        (Some("auto"), "auto"),
        (Some("paste"), "paste"),
        (Some("handler"), "handler"),
    ] {
        let mut args = vec!["ghostlink", "login"];
        if let Some(value) = value {
            args.extend(["--callback", value]);
        }
        let cli = Cli::try_parse_from(args).unwrap();
        let Command::Login(login) = cli.command else {
            panic!("not login")
        };
        assert_eq!(login.callback.as_str(), expected);
    }
    assert_eq!(CallbackMode::default().as_str(), "auto");
}

#[test]
fn handler_commands_parse_with_global_json() {
    for action in ["status", "install", "uninstall"] {
        assert!(Cli::try_parse_from(["ghostlink", "handler", action]).is_ok());
        assert!(
            Cli::try_parse_from(["ghostlink", "handler", action, "--json"])
                .unwrap()
                .json
        );
    }
}
