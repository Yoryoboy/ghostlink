//! Default dashboard over the app's classified groups and Quickplay pools.
use destiny_app::weekly_personal::{PersonalChallenge, PersonalGroup, WeeklyPersonal};
use destiny_app::{Character, DefinitionResolution};

use super::sanitize;

fn label(hash: u32, definition: &DefinitionResolution) -> String {
    match definition {
        DefinitionResolution::Resolved { name, .. } => name.clone(),
        DefinitionResolution::Unresolved => format!("unresolved #{hash}"),
    }
}

fn objective(challenge: &PersonalChallenge) -> String {
    format!(
        "{} {} {}/{}",
        if challenge.complete { "✓" } else { "○" },
        sanitize(&label(challenge.objective_hash, &challenge.definition)),
        challenge.progress,
        challenge.completion_value
    )
}

fn unique_objectives(
    objectives: impl Iterator<Item = PersonalChallenge>,
) -> Vec<PersonalChallenge> {
    let mut unique: Vec<PersonalChallenge> = Vec::new();
    for challenge in objectives {
        if !unique.iter().any(|seen| {
            label(seen.objective_hash, &seen.definition)
                == label(challenge.objective_hash, &challenge.definition)
                && seen.progress == challenge.progress
                && seen.completion_value == challenge.completion_value
                && seen.complete == challenge.complete
        }) {
            unique.push(challenge);
        }
    }
    unique
}

fn unique_names(names: impl Iterator<Item = String>) -> Vec<String> {
    let mut unique = Vec::new();
    for name in names {
        if !unique.contains(&name) {
            unique.push(name);
        }
    }
    unique
}

fn rewards_text(names: &[String]) -> String {
    names
        .iter()
        .map(|name| sanitize(name))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn entity(group: &PersonalGroup, quickplay: bool) -> String {
    let mut lines = vec![sanitize(&group.root_name)];
    if quickplay {
        // Pool membership is supplied by the app; never reclassify raw variants.
        for pool in &group.pools {
            let rewards = unique_names(
                pool.variants
                    .iter()
                    .flat_map(|variant| {
                        variant
                            .rewards
                            .iter()
                            .map(|reward| label(reward.hash, &reward.definition))
                    })
                    .chain(pool.rewards.iter().cloned()),
            );
            let objectives = unique_objectives(pool.objectives.iter().cloned());
            let mut line = objectives
                .iter()
                .map(objective)
                .collect::<Vec<_>>()
                .join("; ");
            if !rewards.is_empty() {
                if !line.is_empty() {
                    line.push_str(" → ");
                }
                line.push_str(&rewards_text(&rewards));
            }
            if !line.is_empty() {
                lines.push(format!("  {line}"));
            }
        }
    } else {
        for challenge in unique_objectives(
            group
                .activities
                .iter()
                .flat_map(|a| a.challenges.iter().cloned()),
        ) {
            lines.push(format!("  {}", objective(&challenge)));
        }
        let mut phases = Vec::new();
        for phase in group.activities.iter().filter_map(|a| a.phases.as_ref()) {
            if !phases.contains(&phase) {
                phases.push(phase);
                lines.push(format!(
                    "  {:<12}{}/{}",
                    "Encounters", phase.complete, phase.total
                ));
            }
        }
        let rewards = unique_names(
            group
                .activities
                .iter()
                .flat_map(|a| a.rewards.iter().map(|r| label(r.hash, &r.definition))),
        );
        if !rewards.is_empty() {
            let heading = if rewards.len() == 1 {
                "Reward"
            } else {
                "Rewards"
            };
            lines.push(format!("  {heading:<12}{}", rewards_text(&rewards)));
        }
    }
    lines.join("\n")
}

pub(super) fn render(character: &Character, weekly: &WeeklyPersonal) -> String {
    let mut character_line = sanitize(character.class.as_deref().unwrap_or("unknown"));
    if let Some(light) = character.light {
        character_line.push_str(&format!(" · Power {light}"));
    }
    let mut blocks = vec![format!("Ghost Link — Weekly\n{character_line}")];
    let grandmaster = weekly
        .vanguard_alerts
        .grandmaster
        .iter()
        .collect::<Vec<_>>();
    let raids = weekly.featured_raids.iter().collect::<Vec<_>>();
    let dungeons = weekly.featured_dungeons.iter().collect::<Vec<_>>();
    let quickplay = weekly.vanguard_alerts.quickplay.iter().collect::<Vec<_>>();
    let crucible = weekly.vanguard_alerts.crucible.iter().collect::<Vec<_>>();
    let seasonal = [
        weekly.weekly_rotators.expedition.as_ref(),
        weekly.weekly_rotators.override_activity.as_ref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    for (heading, groups, pools) in [
        ("GRANDMASTER", grandmaster, false),
        ("FEATURED RAIDS", raids, false),
        ("FEATURED DUNGEONS", dungeons, false),
        ("VANGUARD", quickplay, true),
        ("CRUCIBLE", crucible, false),
        ("SEASONAL", seasonal, false),
    ] {
        if !groups.is_empty() {
            let rendered = groups.iter().map(|g| entity(g, pools)).collect::<Vec<_>>();
            // One-line entities (no detail lines) read as a list; others get a gap.
            let separator = if rendered.iter().all(|e| !e.contains('\n')) {
                "\n"
            } else {
                "\n\n"
            };
            blocks.push(format!("{heading}\n{}", rendered.join(separator)));
        }
    }
    let count = weekly.unclassified.len();
    if count > 0 {
        let noun = if count == 1 { "activity" } else { "activities" };
        blocks.push(format!(
            "⚠ {count} {noun} could not be classified.\n  Run `ghostlink sync-manifest`."
        ));
    }
    blocks.join("\n\n")
}
