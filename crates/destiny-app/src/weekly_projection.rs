//! One provider-neutral weekly presentation contract for CLI and MCP.
use crate::weekly_personal::{ModifierCategory, UnclassifiedReason, UnclassifiedSource};
use crate::{
    Character, DefinitionResolution, PersonalActivity, PersonalChallenge, PersonalGroup,
    WeeklyPersonal,
};

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
fn name(definition: &DefinitionResolution) -> Option<&str> {
    match definition {
        DefinitionResolution::Resolved { name, .. } => Some(name),
        DefinitionResolution::Unresolved => None,
    }
}
fn entity_fields<J: WeeklyJson>(
    hash: u32,
    definition: &DefinitionResolution,
) -> Vec<(&'static str, J::Value)> {
    let name = name(definition);
    vec![
        ("hash", J::integer(hash.into())),
        ("name", optional::<J>(name)),
        ("resolved", J::boolean(name.is_some())),
    ]
}
fn entity<J: WeeklyJson>(hash: u32, definition: &DefinitionResolution) -> J::Value {
    J::object(entity_fields::<J>(hash, definition))
}
fn modifier<J: WeeklyJson>(modifier: &crate::WeeklyModifier) -> J::Value {
    let category = match modifier.category {
        ModifierCategory::Surge => "surge",
        ModifierCategory::Overcharge => "overcharge",
        ModifierCategory::Champions => "champions",
        ModifierCategory::Other => "other",
    };
    let mut fields = entity_fields::<J>(modifier.hash, &modifier.definition);
    fields.push(("category", J::string(category)));
    J::object(fields)
}
fn challenge<J: WeeklyJson>(c: &PersonalChallenge) -> J::Value {
    let mut fields = entity_fields::<J>(c.objective_hash, &c.definition);
    fields.extend([
        ("progress", J::integer(c.progress)),
        ("completion_value", J::integer(c.completion_value)),
        ("complete", J::boolean(c.complete)),
    ]);
    J::object(fields)
}
fn activity_type<J: WeeklyJson>(a: &PersonalActivity) -> J::Value {
    a.activity_type_hash
        .map_or_else(J::null, |hash| entity::<J>(hash, &a.activity_type))
}
fn activity<J: WeeklyJson>(a: &PersonalActivity) -> J::Value {
    let mut fields = entity_fields::<J>(a.hash, &a.definition);
    fields.extend([
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
            J::array(a.modifiers.iter().map(modifier::<J>).collect()),
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
            J::array(a.challenges.iter().map(challenge::<J>).collect()),
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
    ]);
    J::object(fields)
}
fn group_fields<J: WeeklyJson>(g: &PersonalGroup) -> Vec<(&'static str, J::Value)> {
    vec![
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
    ]
}
fn group<J: WeeklyJson>(g: &PersonalGroup) -> J::Value {
    J::object(group_fields::<J>(g))
}
fn quickplay<J: WeeklyJson>(g: &PersonalGroup) -> J::Value {
    let mut fields = group_fields::<J>(g);
    fields.push((
        "pools",
        J::array(
            g.pools
                .iter()
                .map(|pool| {
                    J::object(vec![
                        (
                            "objectives",
                            J::array(pool.objectives.iter().map(challenge::<J>).collect()),
                        ),
                        (
                            "rewards",
                            J::array(pool.rewards.iter().map(|n| J::string(n)).collect()),
                        ),
                        (
                            "variants",
                            J::array(pool.variants.iter().map(activity::<J>).collect()),
                        ),
                    ])
                })
                .collect(),
        ),
    ));
    J::object(fields)
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
                (
                    "quickplay",
                    J::array(
                        weekly
                            .vanguard_alerts
                            .quickplay
                            .iter()
                            .map(quickplay::<J>)
                            .collect(),
                    ),
                ),
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
            "unclassified",
            J::array(
                weekly
                    .unclassified
                    .iter()
                    .map(|a| {
                        J::object(vec![
                            ("hash", J::integer(a.hash.into())),
                            ("name", optional::<J>(a.name.as_deref())),
                            (
                                "reason",
                                J::string(match a.reason {
                                    UnclassifiedReason::ActivityDefinitionMissing => {
                                        "activity_definition_missing"
                                    }
                                    UnclassifiedReason::ActivityTypeMissing => {
                                        "activity_type_missing"
                                    }
                                }),
                            ),
                            (
                                "source",
                                J::string(match a.source {
                                    UnclassifiedSource::PublicMilestones => "public_milestones",
                                    UnclassifiedSource::CharacterActivities => {
                                        "character_activities"
                                    }
                                }),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "unclassified_unresolved",
            J::integer(weekly.unclassified.len() as i64),
        ),
    ])
}
