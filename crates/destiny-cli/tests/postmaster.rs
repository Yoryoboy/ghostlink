use clap::Parser;
use destiny_cli::{Cli, ReadRequest, parse_process};

#[test]
fn postmaster_parse_all_selectors_and_global_json() {
    for args in [
        vec!["ghostlink", "postmaster"],
        vec!["ghostlink", "--json", "postmaster"],
        vec!["ghostlink", "postmaster", "--json"],
    ] {
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(
            cli.into_read_request().unwrap(),
            ReadRequest::Postmaster(None)
        );
    }
    for selector in ["123", "HuNtEr"] {
        let cli =
            Cli::try_parse_from(["ghostlink", "postmaster", "--character", selector]).unwrap();
        assert_eq!(
            cli.into_read_request().unwrap(),
            ReadRequest::Postmaster(Some(destiny_app::CharacterId::new(selector)))
        );
    }
}

#[test]
fn postmaster_rejects_invalid_arguments_without_echoing_them() {
    for extra in [
        vec!["extra"],
        vec!["--pull"],
        vec!["--quantity", "1"],
        vec!["--character", "123", "--character", "456"],
        vec!["--unknown"],
        vec!["--vault"],
    ] {
        let mut args = vec!["ghostlink", "postmaster", "--json"];
        args.extend(extra);
        let output = parse_process(args).unwrap_err();
        assert_eq!(output.exit_code, 2);
        assert!(output.stdout.is_empty());
        assert!(output.stderr.contains("invalid_request"));
        assert!(!output.stderr.contains("456"));
    }
    for selector in [String::new(), " ".into(), "x\n".into(), "x".repeat(257)] {
        let cli =
            Cli::try_parse_from(["ghostlink", "postmaster", "--character", &selector]).unwrap();
        assert!(cli.into_read_request().is_err());
    }
    let bounded = "x".repeat(destiny_app::MAX_SEARCH_TEXT_LENGTH);
    let cli = Cli::try_parse_from(["ghostlink", "postmaster", "--character", &bounded]).unwrap();
    assert!(cli.into_read_request().is_ok());
}

#[test]
fn postmaster_help_is_success_without_runtime() {
    let help = parse_process(["ghostlink", "postmaster", "--help"]).unwrap_err();
    assert_eq!(help.exit_code, 0);
    assert!(help.stdout.contains("--character"));
    assert!(help.stdout.contains("--json"));
    assert!(help.stderr.is_empty());
}
