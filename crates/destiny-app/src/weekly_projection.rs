//! One provider-neutral weekly presentation contract for CLI and MCP.
use crate::{Character, DefinitionResolution, PersonalActivity, PersonalGroup, WeeklyPersonal};

/// Presentation adapters supply their JSON representation, not classification policy.
pub trait WeeklyJson {
    type Value;
    fn object(fields: Vec<(&'static str, Self::Value)>) -> Self::Value;
    fn array(values: Vec<Self::Value>) -> Self::Value;
    fn string(value: &str) -> Self::Value;
    fn integer(value: i64) -> Self::Value;
    fn boolean(value: bool) -> Self::Value;
    fn null() -> Self::Value;
}
fn optional<J: WeeklyJson>(value: Option<&str>) -> J::Value {
    value.map_or_else(J::null, J::string)
}
fn entity<J: WeeklyJson>(hash: u32, definition: &DefinitionResolution) -> J::Value {
    let name = match definition {
        DefinitionResolution::Resolved { name, .. } => Some(name.as_str()),
        DefinitionResolution::Unresolved => None,
    };
    J::object(vec![
        ("hash", J::integer(hash.into())),
        ("name", optional::<J>(name)),
        ("resolved", J::boolean(name.is_some())),
    ])
}
fn activity_type<J: WeeklyJson>(a: &PersonalActivity) -> J::Value {
    a.activity_type_hash
        .map_or_else(J::null, |hash| entity::<J>(hash, &a.activity_type))
}
fn activity<J: WeeklyJson>(a: &PersonalActivity) -> J::Value {
    let name = match &a.definition {
        DefinitionResolution::Resolved { name, .. } => Some(name.as_str()),
        DefinitionResolution::Unresolved => None,
    };
    J::object(vec![
        ("hash", J::integer(a.hash.into())),
        ("name", optional::<J>(name)),
        ("resolved", J::boolean(name.is_some())),
        ("activity_type", activity_type::<J>(a)),
        (
            "rewards",
            J::array(
                a.rewards
                    .iter()
                    .map(|r| entity::<J>(r.hash, &r.definition))
                    .collect(),
            ),
        ),
        (
            "modifiers",
            J::array(
                a.modifiers
                    .iter()
                    .map(|m| entity::<J>(m.hash, &m.definition))
                    .collect(),
            ),
        ),
        (
            "challenge_objectives",
            J::array(
                a.challenge_objectives
                    .iter()
                    .map(|o| entity::<J>(o.hash, &o.definition))
                    .collect(),
            ),
        ),
        (
            "challenges",
            J::array(
                a.challenges
                    .iter()
                    .map(|c| {
                        let name = match &c.definition {
                            DefinitionResolution::Resolved { name, .. } => Some(name.as_str()),
                            DefinitionResolution::Unresolved => None,
                        };
                        J::object(vec![
                            ("hash", J::integer(c.objective_hash.into())),
                            ("name", optional::<J>(name)),
                            ("resolved", J::boolean(name.is_some())),
                            ("progress", J::integer(c.progress)),
                            ("completion_value", J::integer(c.completion_value)),
                            ("complete", J::boolean(c.complete)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "phases",
            a.phases.as_ref().map_or_else(J::null, |p| {
                J::object(vec![
                    ("complete", J::integer(p.complete as i64)),
                    ("total", J::integer(p.total as i64)),
                ])
            }),
        ),
    ])
}
fn group<J: WeeklyJson>(g: &PersonalGroup) -> J::Value {
    J::object(vec![
        ("root", J::string(&g.root_name)),
        (
            "reward_names",
            J::array(g.reward_names.iter().map(|n| J::string(n)).collect()),
        ),
        (
            "activity_type",
            g.activities
                .first()
                .map_or_else(J::null, activity_type::<J>),
        ),
        (
            "variants",
            J::array(g.activities.iter().map(activity::<J>).collect()),
        ),
    ])
}
fn groups<J: WeeklyJson>(g: &[PersonalGroup]) -> J::Value {
    J::array(g.iter().map(group::<J>).collect())
}

/// Projects only app-classified groups; derived roots never acquire invented hashes.
pub fn weekly_projection<J: WeeklyJson>(
    character: &Character,
    weekly: &WeeklyPersonal,
) -> J::Value {
    J::object(vec![
        ("kind", J::string("weekly")),
        (
            "character",
            J::object(vec![
                ("id", J::string(character.id.as_str())),
                ("name", optional::<J>(character.name.as_deref())),
                ("class", optional::<J>(character.class.as_deref())),
                ("race", optional::<J>(character.race.as_deref())),
                ("gender", optional::<J>(character.gender.as_deref())),
                (
                    "light",
                    character
                        .light
                        .map_or_else(J::null, |n| J::integer(n.into())),
                ),
            ]),
        ),
        (
            "definition_version",
            J::string(weekly.definition_version.as_str()),
        ),
        ("featured_raids", groups::<J>(&weekly.featured_raids)),
        ("featured_dungeons", groups::<J>(&weekly.featured_dungeons)),
        (
            "vanguard_alerts",
            J::object(vec![
                (
                    "grandmaster",
                    weekly
                        .vanguard_alerts
                        .grandmaster
                        .as_ref()
                        .map_or_else(J::null, group::<J>),
                ),
                ("crucible", groups::<J>(&weekly.vanguard_alerts.crucible)),
                ("quickplay", groups::<J>(&weekly.vanguard_alerts.quickplay)),
            ]),
        ),
        (
            "expedition",
            weekly
                .weekly_rotators
                .expedition
                .as_ref()
                .map_or_else(J::null, group::<J>),
        ),
        (
            "override",
            weekly
                .weekly_rotators
                .override_activity
                .as_ref()
                .map_or_else(J::null, group::<J>),
        ),
        (
            "milestones",
            J::array(
                weekly
                    .milestones
                    .iter()
                    .map(|m| entity::<J>(m.hash, &m.definition))
                    .collect(),
            ),
        ),
        (
            "unclassified_unresolved",
            J::integer(weekly.unclassified_unresolved as i64),
        ),
    ])
}
