//! Keeps the agent-facing skill aligned with the real clap command surface.
//!
//! The skill at `skills/ghostlink/SKILL.md` names commands and flags inside
//! code spans and fenced blocks. Every `ghostlink <command>` it mentions must
//! exist, every command must be mentioned, and every `--flag` it writes next to
//! a command must be accepted by that command. Placeholders such as `<cmd>` are
//! ignored so the skill can describe the `--help` escape hatch generically.

use std::collections::BTreeSet;
use std::path::PathBuf;

use clap::CommandFactory;
use destiny_cli::Cli;

fn skill_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills/ghostlink/SKILL.md")
}

fn read_skill() -> String {
    let path = skill_path();
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read skill at {}: {error}", path.display()))
}

/// Code snippets: inline code spans plus every line inside fenced blocks.
fn snippets(markdown: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut fenced = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            found.push(line.trim().to_owned());
            continue;
        }
        found.extend(
            line.split('`')
                .skip(1)
                .step_by(2)
                .map(|span| span.trim().to_owned()),
        );
    }
    found
}

struct Invocation {
    command: Option<String>,
    flags: Vec<String>,
}

fn invocations(markdown: &str) -> Vec<Invocation> {
    snippets(markdown)
        .iter()
        .filter_map(|snippet| {
            let mut tokens = snippet.split_whitespace();
            if tokens.next() != Some("ghostlink") {
                return None;
            }
            let rest: Vec<&str> = tokens.collect();
            let command = rest
                .first()
                .filter(|token| !token.starts_with('-') && !token.starts_with('<'))
                .map(|token| (*token).to_owned());
            let flags = rest
                .iter()
                .filter_map(|token| token.strip_prefix("--"))
                .map(|flag| flag.split('=').next().unwrap_or(flag).to_owned())
                .collect();
            Some(Invocation { command, flags })
        })
        .collect()
}

/// Public subcommands; hidden ones (such as the URI-handler `callback` entry)
/// are not for agents and must not be required by the skill.
fn subcommands() -> BTreeSet<String> {
    Cli::command()
        .get_subcommands()
        .filter(|command| !command.is_hide_set())
        .map(|command| command.get_name().to_owned())
        .filter(|name| name != "help")
        .collect()
}

fn accepted_flags(command: Option<&str>) -> BTreeSet<String> {
    let mut root = Cli::command();
    root.build();
    let longs = |command: &clap::Command| -> Vec<String> {
        command
            .get_arguments()
            .filter_map(|argument| argument.get_long().map(str::to_owned))
            .collect()
    };
    // After `build`, global root flags are propagated into every subcommand,
    // so a subcommand's own arguments are exactly what it accepts; root-only
    // flags such as `--version` stay on the root.
    let mut flags: BTreeSet<String> = match command {
        Some(name) => root
            .find_subcommand(name)
            .map(longs)
            .unwrap_or_default()
            .into_iter()
            .collect(),
        None => longs(&root).into_iter().collect(),
    };
    flags.insert("help".to_owned());
    flags
}

#[test]
fn skill_mentions_only_real_commands() {
    let known = subcommands();
    let unknown: BTreeSet<String> = invocations(&read_skill())
        .into_iter()
        .filter_map(|invocation| invocation.command)
        .filter(|command| !known.contains(command))
        .collect();
    assert!(
        unknown.is_empty(),
        "skill names unknown commands: {unknown:?}"
    );
}

#[test]
fn skill_covers_every_command() {
    let mentioned: BTreeSet<String> = invocations(&read_skill())
        .into_iter()
        .filter_map(|invocation| invocation.command)
        .collect();
    // Integration bootstrap is operator-facing, not an in-game agent operation.
    // Its command contracts are covered by tests/agents.rs, tests/setup.rs,
    // and docs/cli.md.
    let missing: BTreeSet<String> = subcommands()
        .difference(&mentioned)
        .filter(|name| !matches!(name.as_str(), "agents" | "setup"))
        .cloned()
        .collect();
    assert!(
        missing.is_empty(),
        "skill does not cover commands: {missing:?}"
    );
}

#[test]
fn skill_flags_exist_on_their_command() {
    let mut invalid = BTreeSet::new();
    for invocation in invocations(&read_skill()) {
        let accepted = accepted_flags(invocation.command.as_deref());
        for flag in invocation.flags {
            if !accepted.contains(&flag) {
                let command = invocation.command.as_deref().unwrap_or("<root>");
                invalid.insert(format!("{command} --{flag}"));
            }
        }
    }
    assert!(invalid.is_empty(), "skill uses unknown flags: {invalid:?}");
}

#[test]
fn subcommands_accept_global_but_not_root_only_flags() {
    let sync = accepted_flags(Some("sync"));
    assert!(sync.contains("json"));
    assert!(!sync.contains("version"));
    assert!(accepted_flags(None).contains("version"));
}

#[test]
fn action_contract_describes_local_state_not_post_action_sync() {
    let skill = read_skill();
    let exits = std::fs::read_to_string(
        skill_path()
            .parent()
            .unwrap()
            .join("references/exit-codes.md"),
    )
    .unwrap();
    for text in [&skill, &exits] {
        assert!(text.contains("`local_state`"));
        assert!(text.contains("Ghostlink's acknowledged actions"));
        assert!(text.contains("nothing was sent to Bungie"));
        assert!(!text.contains("forces sync"));
        assert!(!text.contains("local_state_updated"));
    }
}

#[test]
fn snippet_parser_reads_spans_and_fences() {
    let markdown = "Run `ghostlink sync --json` first.\n```sh\nghostlink equip X --character hunter\n```\n`ghostlink <cmd> --help`";
    let parsed = invocations(markdown);
    let commands: Vec<Option<&str>> = parsed.iter().map(|i| i.command.as_deref()).collect();
    assert_eq!(commands, vec![Some("sync"), Some("equip"), None]);
    assert_eq!(parsed[1].flags, vec!["character".to_owned()]);
}
