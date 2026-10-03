//! Default dashboard over the app's classified groups and Quickplay pools.
use destiny_app::weekly_personal::{PersonalChallenge, PersonalGroup, WeeklyPersonal};
use destiny_app::{Character, DefinitionResolution};

use super::sanitize;
pub(super) use crate::command::WeeklyLevel as Level;

pub(super) fn render_level(character: &Character, weekly: &WeeklyPersonal, level: Level) -> String {
    dashboard(character, weekly, level)
}

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

fn diagnostic_label(hash: u32, definition: &DefinitionResolution) -> String {
    match definition {
        DefinitionResolution::Resolved { name, .. } => format!("{} #{hash}", sanitize(name)),
        DefinitionResolution::Unresolved => format!("unresolved #{hash} (unresolved)"),
    }
}

fn modifier_lines(
    modifiers: &[&destiny_app::WeeklyModifier],
    indent: &str,
    debug: bool,
) -> Vec<String> {
    use destiny_app::weekly_personal::ModifierCategory;
    let mut lines = Vec::new();
    for (category, heading) in [
        (ModifierCategory::Surge, "Surge"),
        (ModifierCategory::Overcharge, "Overcharge"),
        (ModifierCategory::Champions, "Champions"),
        (ModifierCategory::Other, "Other modifiers"),
    ] {
        let names = modifiers
            .iter()
            .filter(|modifier| modifier.category == category)
            .map(|modifier| {
                if debug {
                    diagnostic_label(modifier.hash, &modifier.definition)
                } else {
                    label(modifier.hash, &modifier.definition)
                }
            })
            .collect::<Vec<_>>();
        let names = if debug {
            names
        } else {
            unique_names(names.into_iter())
        };
        if !names.is_empty() {
            lines.push(format!("{indent}{heading:<17}{}", rewards_text(&names)));
        }
    }
    lines
}

fn variant_suffix(activity: &destiny_app::weekly_personal::PersonalActivity, root: &str) -> String {
    let name = label(activity.hash, &activity.definition);
    let suffix = name.strip_prefix(root).unwrap_or(&name).trim();
    let suffix = suffix
        .trim_start_matches(':')
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')');
    sanitize(if suffix.is_empty() { &name } else { suffix })
}

fn visible_modifier_names(
    activity: &destiny_app::weekly_personal::PersonalActivity,
) -> Vec<String> {
    unique_names(
        activity
            .modifiers
            .iter()
            .filter_map(|modifier| match &modifier.definition {
                DefinitionResolution::Resolved { name, .. } => Some(name.clone()),
                DefinitionResolution::Unresolved => None,
            }),
    )
}

fn details_modifiers(activities: &[destiny_app::weekly_personal::PersonalActivity]) -> Vec<String> {
    let Some(first) = activities.first() else {
        return Vec::new();
    };
    // Intersection uses exact display names, not hashes or provider ordering.
    let common = first
        .modifiers
        .iter()
        .filter(|modifier| {
            if modifier.definition == DefinitionResolution::Unresolved {
                return false;
            }
            let name = label(modifier.hash, &modifier.definition);
            activities.iter().all(|activity| {
                activity
                    .modifiers
                    .iter()
                    .any(|other| label(other.hash, &other.definition) == name)
            })
        })
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    if !common.is_empty() {
        lines.push("  Modifiers".into());
        lines.extend(modifier_lines(&common, "    ", false));
    }
    for activity in activities {
        let extra = activity
            .modifiers
            .iter()
            .filter(|modifier| {
                modifier.definition != DefinitionResolution::Unresolved
                    && !common.iter().any(|other| {
                        label(other.hash, &other.definition)
                            == label(modifier.hash, &modifier.definition)
                    })
            })
            .collect::<Vec<_>>();
        if !extra.is_empty() {
            if common.is_empty() && lines.is_empty() {
                lines.push("  Modifiers".into());
            }
            let root = destiny_app::weekly_personal::activity_root(match &activity.definition {
                DefinitionResolution::Resolved { name, .. } => name,
                DefinitionResolution::Unresolved => "",
            });
            lines.push(format!("  {}", variant_suffix(activity, root)));
            lines.extend(modifier_lines(&extra, "    ", false));
        }
    }
    lines
}

fn debug_activity(activity: &destiny_app::weekly_personal::PersonalActivity) -> Vec<String> {
    let mut lines = vec![format!(
        "  {}",
        diagnostic_label(activity.hash, &activity.definition).replacen(" #", "  #", 1)
    )];
    for challenge in &activity.challenges {
        lines.push(format!(
            "    {} {} {}/{}  #{}{}",
            if challenge.complete { "✓" } else { "○" },
            match &challenge.definition {
                DefinitionResolution::Resolved { name, .. } => sanitize(name),
                DefinitionResolution::Unresolved => "unresolved".into(),
            },
            challenge.progress,
            challenge.completion_value,
            challenge.objective_hash,
            if challenge.definition == DefinitionResolution::Unresolved {
                " (unresolved)"
            } else {
                ""
            }
        ));
    }
    if let Some(phase) = &activity.phases {
        lines.push(format!(
            "    {:<12}{}/{}",
            "Encounters", phase.complete, phase.total
        ));
    }
    if !activity.rewards.is_empty() {
        let rewards = activity
            .rewards
            .iter()
            .map(|reward| diagnostic_label(reward.hash, &reward.definition))
            .collect::<Vec<_>>();
        lines.push(format!("    {:<17}{}", "Rewards", rewards_text(&rewards)));
    }
    if !activity.modifiers.is_empty() {
        lines.push("    Modifiers".into());
        lines.extend(modifier_lines(
            &activity.modifiers.iter().collect::<Vec<_>>(),
            "      ",
            true,
        ));
    }
    lines
}

fn entity_level(group: &PersonalGroup, quickplay: bool, level: Level) -> String {
    if level == Level::Default {
        return entity(group, quickplay);
    }
    if level == Level::Debug {
        let hashes = group
            .activities
            .iter()
            .map(|activity| format!("#{}", activity.hash))
            .collect::<Vec<_>>();
        let mut lines = vec![format!(
            "{}  {}",
            sanitize(&group.root_name),
            hashes.join(" · ")
        )];
        if quickplay && !group.pools.is_empty() {
            for (index, pool) in group.pools.iter().enumerate() {
                let hashes = pool
                    .variants
                    .iter()
                    .map(|variant| format!("#{}", variant.hash))
                    .collect::<Vec<_>>();
                lines.push(format!("  Pool {}  {}", index + 1, hashes.join(" · ")));
                for variant in &pool.variants {
                    lines.extend(debug_activity(variant));
                }
            }
        } else {
            for activity in &group.activities {
                lines.extend(debug_activity(activity));
            }
        }
        return lines.join("\n");
    }
    let mut text = entity(group, quickplay);
    let mut lines = Vec::new();
    if quickplay && !group.pools.is_empty() {
        let variants = group
            .pools
            .iter()
            .flat_map(|pool| pool.variants.iter().cloned())
            .collect::<Vec<_>>();
        let shared = variants.first().is_none_or(|first| {
            let names = visible_modifier_names(first);
            variants.iter().all(|variant| {
                let other = visible_modifier_names(variant);
                names.len() == other.len() && names.iter().all(|name| other.contains(name))
            })
        });
        if shared {
            lines = details_modifiers(&variants);
        } else {
            for pool in &group.pools {
                let modifiers = details_modifiers(&pool.variants);
                if !modifiers.is_empty() {
                    lines.push(format!("  → {}", rewards_text(&pool.rewards)));
                    lines.extend(modifiers);
                }
            }
        }
    } else {
        lines = details_modifiers(&group.activities);
    }
    if !lines.is_empty() {
        text.push('\n');
        text.push_str(&lines.join("\n"));
    }
    text
}

pub(super) fn render(character: &Character, weekly: &WeeklyPersonal) -> String {
    dashboard(character, weekly, Level::Default)
}

fn dashboard(character: &Character, weekly: &WeeklyPersonal, level: Level) -> String {
    let mut character_line = sanitize(character.class.as_deref().unwrap_or("unknown"));
    if let Some(light) = character.light {
        character_line.push_str(&format!(" · Power {light}"));
    }
    let mut header = format!("Ghost Link — Weekly\n{character_line}");
    if level != Level::Default {
        header.push_str("\nModifier groups are derived from names.");
    }
    if level == Level::Debug {
        header.push_str(&format!(
            "\nManifest {}",
            sanitize(weekly.definition_version.as_str())
        ));
    }
    let mut blocks = vec![header];
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
    let mut hidden = std::collections::BTreeSet::new();
    for (heading, groups, pools) in [
        ("GRANDMASTER", grandmaster, false),
        ("FEATURED RAIDS", raids, false),
        ("FEATURED DUNGEONS", dungeons, false),
        ("VANGUARD", quickplay, true),
        ("CRUCIBLE", crucible, false),
        ("SEASONAL", seasonal, false),
    ] {
        if !groups.is_empty() {
            if level == Level::Details {
                for group in &groups {
                    let activities = if pools && !group.pools.is_empty() {
                        group
                            .pools
                            .iter()
                            .flat_map(|pool| pool.variants.iter())
                            .collect::<Vec<_>>()
                    } else {
                        group.activities.iter().collect::<Vec<_>>()
                    };
                    for activity in activities {
                        for modifier in &activity.modifiers {
                            if modifier.definition == DefinitionResolution::Unresolved {
                                hidden.insert(modifier.hash);
                            }
                        }
                    }
                }
            }
            let rendered = groups
                .iter()
                .map(|g| entity_level(g, pools, level))
                .collect::<Vec<_>>();
            // One-line entities (no detail lines) read as a list; others get a gap.
            let separator = if rendered.iter().all(|e| !e.contains('\n')) {
                "\n"
            } else {
                "\n\n"
            };
            blocks.push(format!("{heading}\n{}", rendered.join(separator)));
        }
    }
    if !hidden.is_empty() {
        let count = hidden.len();
        let noun = if count == 1 { "modifier" } else { "modifiers" };
        blocks.push(format!("{count} unnamed {noun} hidden; use --debug."));
    }
    let count = weekly.unclassified.len();
    if count > 0 && level == Level::Debug {
        use destiny_app::weekly_personal::{UnclassifiedReason, UnclassifiedSource};
        let entries = weekly
            .unclassified
            .iter()
            .map(|entry| {
                let reason = match entry.reason {
                    UnclassifiedReason::ActivityDefinitionMissing => "activity_definition_missing",
                    UnclassifiedReason::ActivityTypeMissing => "activity_type_missing",
                };
                let source = match entry.source {
                    UnclassifiedSource::PublicMilestones => "public_milestones",
                    UnclassifiedSource::CharacterActivities => "character_activities",
                };
                format!(
                    "Activity {}\n  name    {}\n  reason  {reason}\n  source  {source}",
                    entry.hash,
                    sanitize(entry.name.as_deref().unwrap_or("unknown"))
                )
            })
            .collect::<Vec<_>>();
        blocks.push(format!("UNCLASSIFIED\n{}", entries.join("\n\n")));
    } else if count > 0 {
        let noun = if count == 1 { "activity" } else { "activities" };
        blocks.push(format!(
            "⚠ {count} {noun} could not be classified.\n  Run `ghostlink sync-manifest`."
        ));
    }
    blocks.join("\n\n")
}
