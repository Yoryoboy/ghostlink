//! Cheap English-text drift guard, not a language detector.
//! Scan only public presentation sources and documentation, not external
//! Manifest/Clarity data or fixture directories outside these roots.
use std::fs;
use std::path::{Path, PathBuf};

const ROOTS: &[&str] = &[
    "crates/destiny-output/src",
    "crates/destiny-cli/src",
    "crates/destiny-mcp/src",
    "docs",
    "skills/ghostlink",
    "README.md",
];
const SPANISH_CHARACTERS: &str =
    "\u{e1}\u{e9}\u{ed}\u{f3}\u{fa}\u{f1}\u{bf}\u{a1}\u{c1}\u{c9}\u{cd}\u{d3}\u{da}\u{d1}";

// No exceptions currently. Future entries must identify an exact relative path
// and exact line text, with a reason (e.g. a proper name or verbatim fixture).
// Never exempt an entire crate or documentation tree. Stale entries fail too.
const ALLOW_LIST: &[(&str, &str, &str)] = &[];

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) {
    let metadata = fs::symlink_metadata(path).expect("scan path must exist");
    assert!(
        !metadata.file_type().is_symlink(),
        "unexpected symlink: {}",
        path.display()
    );
    if metadata.is_dir() {
        for entry in fs::read_dir(path).expect("scan directory must be readable") {
            collect_files(
                &entry.expect("directory entry must be readable").path(),
                files,
            );
        }
    } else {
        files.push(path.to_owned());
    }
}

fn violations(path: &str, text: &str) -> Vec<String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| line.chars().any(|ch| SPANISH_CHARACTERS.contains(ch)))
        .filter(|(_, line)| {
            !ALLOW_LIST
                .iter()
                .any(|(allowed_path, allowed_line, reason)| {
                    assert!(
                        !reason.trim().is_empty(),
                        "allow-list entries need a justification"
                    );
                    path == *allowed_path && *line == *allowed_line
                })
        })
        .map(|(index, _)| format!("{path}:{}", index + 1))
        .collect()
}

#[test]
fn public_text_has_no_spanish_character_drift() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for path in ROOTS {
        collect_files(&root.join(path), &mut files);
    }
    files.sort();
    let mut hits = Vec::new();
    let mut used = vec![false; ALLOW_LIST.len()];
    for file in files {
        let path = file.strip_prefix(&root).unwrap().to_str().unwrap();
        let text = fs::read_to_string(&file).expect("public source and docs must be UTF-8");
        hits.extend(violations(path, &text));
        for (index, (allowed_path, allowed_line, _)) in ALLOW_LIST.iter().enumerate() {
            if path == *allowed_path && text.lines().any(|line| line == *allowed_line) {
                used[index] = true;
            }
        }
    }
    assert!(
        hits.is_empty(),
        "Non-English text candidates (translate or justify an exact allow-list entry):\n{}",
        hits.join("\n")
    );
    assert!(
        used.iter().all(|used| *used),
        "remove stale allow-list entries"
    );
}

#[test]
fn guard_detects_every_character_and_reports_file_and_line() {
    for character in SPANISH_CHARACTERS.chars() {
        assert_eq!(
            violations("docs/example.md", &format!("English\n{character}\n")),
            ["docs/example.md:2"]
        );
    }
    assert!(violations("docs/example.md", "English-only text.\n").is_empty());
}
