//! `build substitutes` / MCP `build_substitutes`: one JSON shape for both surfaces and the
//! human text rendered from it.
use crate::builds::{clean_text, metadata_json, summary_json};
use destiny_app::build_substitutes::SubstituteMod;
use destiny_app::builds::SubstitutesResult;
use serde_json::{Value, json};

const BUCKET_LABELS: [&str; 5] = ["Helmet", "Gauntlets", "Chest", "Legs", "Class item"];

fn mods(mods: &[SubstituteMod]) -> Value {
    json!(
        mods.iter()
            .map(|m| json!({"hash":m.hash, "name":m.name}))
            .collect::<Vec<_>>()
    )
}

pub(crate) fn json(result: &SubstitutesResult) -> Value {
    let report = &result.report;
    let info = &report.optimizer;
    let sets: Vec<_> = report
        .sets
        .iter()
        .enumerate()
        .map(|(rank, set)| {
            json!({"rank":rank + 1, "power":set.power,
                "pieces":set.pieces.iter().map(|p| json!({"instance_id":p.instance_id, "hash":p.hash,
                    "name":p.name, "bucket_hash":p.bucket_hash, "in_build":p.in_build})).collect::<Vec<_>>(),
                "stats":set.stats.iter().map(|s| json!({"stat_hash":s.stat_hash, "name":s.name,
                    "total":s.total, "armor_only":s.armor_only})).collect::<Vec<_>>(),
                "stat_mods":mods(&set.stat_mods), "tuning_mods":mods(&set.tuning_mods)})
        })
        .collect();
    json!({"kind":"build_substitutes", "summary":summary_json(&result.summary),
        "target_character":result.target_character.as_str(),
        "evaluation":report.evaluation, "reason":report.reason, "limit":report.limit, "sets":sets,
        "optimizer":{"evaluation":info.evaluation, "reason":info.reason,
            "candidates_per_bucket":info.candidates, "combinations_total":info.combos_total,
            "combinations_visited":info.combos_visited, "truncated":info.truncated, "threads":info.threads},
        "metadata":metadata_json(&result.metadata)})
}

/// Mods with their repeat counts, in first-seen order: "Name x2, Other".
fn mod_line(label: &str, mods: &[Value]) -> Option<String> {
    let mut counted: Vec<(String, usize)> = Vec::new();
    for m in mods {
        let name = match m["name"].as_str() {
            Some(name) => clean_text(name),
            None => m["hash"].to_string(),
        };
        match counted.iter_mut().find(|(existing, _)| *existing == name) {
            Some((_, count)) => *count += 1,
            None => counted.push((name, 1)),
        }
    }
    (!counted.is_empty()).then(|| {
        let list: Vec<_> = counted
            .into_iter()
            .map(|(name, count)| {
                if count > 1 {
                    format!("{name} x{count}")
                } else {
                    name
                }
            })
            .collect();
        format!("  {label}: {}", list.join(", "))
    })
}

pub(crate) fn render(v: &Value) -> String {
    let sets = v["sets"].as_array().map_or(&[][..], Vec::as_slice);
    if v["evaluation"] != "evaluated" {
        return format!(
            "Substitutes not evaluated: {}",
            v["reason"].as_str().unwrap_or("unknown")
        );
    }
    let optimizer = &v["optimizer"];
    let mut lines = vec![
        format!(
            "Substitutes for {} on character {}: {} {} (limit {})",
            clean_text(v["summary"]["name"].as_str().unwrap_or("")),
            clean_text(v["target_character"].as_str().unwrap_or("")),
            sets.len(),
            if sets.len() == 1 { "set" } else { "sets" },
            v["limit"]
        ),
        format!(
            "Optimizer: candidates per bucket: {}, combinations visited: {} of {}, threads: {}",
            optimizer["candidates_per_bucket"],
            optimizer["combinations_visited"],
            optimizer["combinations_total"],
            optimizer["threads"]
        ),
    ];
    if optimizer["truncated"] == true {
        lines.push(
            "Truncated: the time budget ran out; these are the best sets found so far.".into(),
        );
    }
    if sets.is_empty() {
        lines.push("No armor sets match the saved stat ranges.".into());
    }
    for set in sets {
        lines.push(format!("#{}  power {}", set["rank"], set["power"]));
        for (label, piece) in BUCKET_LABELS
            .iter()
            .zip(set["pieces"].as_array().into_iter().flatten())
        {
            lines.push(format!(
                "  {label}: {} ({}){}",
                clean_text(piece["name"].as_str().unwrap_or("")),
                clean_text(piece["instance_id"].as_str().unwrap_or("")),
                if piece["in_build"] == true {
                    ""
                } else {
                    " [not in build]"
                }
            ));
        }
        let stats: Vec<_> = set["stats"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| {
                let name = s["name"]
                    .as_str()
                    .map_or_else(|| s["stat_hash"].to_string(), clean_text);
                format!("{name} {} (armor {})", s["total"], s["armor_only"])
            })
            .collect();
        lines.push(format!("  Stats: {}", stats.join(", ")));
        let list = |key: &str| set[key].as_array().map_or(&[][..], Vec::as_slice).to_vec();
        lines.extend(mod_line("Stat mods", &list("stat_mods")));
        lines.extend(mod_line("Tuning mods", &list("tuning_mods")));
    }
    lines.join("\n")
}
