//! GetCharacter components 202/204 only; unknown provider fields are ignored.
use destiny_core::*;
use serde::{
    Deserialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use std::{collections::HashSet, fmt};

pub const WEEKLY_RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
fn invalid() -> AppError {
    AppError::new(ErrorCategory::ProviderValidation)
}

/// Canonical identifiers only, before any credential is attached.
pub fn weekly_profile_endpoint(
    membership: &DestinyMembership,
    character: &CharacterId,
) -> Result<String, AppError> {
    let url = format!(
        "https://www.bungie.net/Platform/Destiny2/{}/Profile/{}/Character/{}/?components=202,204",
        membership.membership_type().value(),
        membership.membership_id().as_str(),
        character.as_str()
    );
    if !is_allowed_weekly_url(&url) {
        return Err(invalid());
    }
    Ok(url)
}
pub(crate) fn is_allowed_weekly_url(url: &str) -> bool {
    let Some(path) = url.strip_prefix("https://www.bungie.net/Platform/Destiny2/") else {
        return false;
    };
    let parts: Vec<_> = path.split('/').collect();
    let canonical = |s: &str| {
        !s.is_empty()
            && s.len() <= 128
            && s.bytes().all(|b| b.is_ascii_digit())
            && (s == "0" || !s.starts_with('0'))
    };
    parts.len() == 6
        && canonical(parts[0])
        && parts[0].parse::<u32>().is_ok_and(|v| v != 0 && v != 254)
        && parts[1] == "Profile"
        && canonical(parts[2])
        && parts[3] == "Character"
        && canonical(parts[4])
        && parts[5] == "?components=202,204"
}

// Bounds are enforced while deserializing, not after allocating arbitrary lists.
struct List<T, const N: usize>(Vec<T>);
impl<T, const N: usize> Default for List<T, N> {
    fn default() -> Self {
        Self(Vec::new())
    }
}
impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for List<T, N> {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T, const N: usize>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for V<T, N> {
            type Value = List<T, N>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded list")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element()? {
                    if values.len() == N {
                        return Err(de::Error::custom("list limit"));
                    }
                    values.push(value);
                }
                Ok(List(values))
            }
        }
        d.deserialize_seq(V::<T, N>(std::marker::PhantomData))
    }
}
type Nested<T> = List<T, 64>;
#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "ErrorCode")]
    code: u32,
    #[serde(rename = "Response")]
    response: Response,
}
#[derive(Deserialize)]
struct Response {
    progressions: Component<Progressions>,
    activities: Component<Activities>,
}
#[derive(Deserialize)]
struct Component<T> {
    data: T,
}
#[derive(Deserialize)]
struct Progressions {
    milestones: Milestones,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Activities {
    available_activities: List<Activity, 512>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Objective {
    objective_hash: u32,
    progress: i64,
    completion_value: i64,
    complete: bool,
}
#[derive(Deserialize)]
struct Challenge {
    objective: Objective,
}
impl From<Challenge> for ChallengeProgress {
    fn from(c: Challenge) -> Self {
        Self {
            objective_hash: c.objective.objective_hash,
            progress: c.objective.progress,
            completion_value: c.objective.completion_value,
            complete: c.objective.complete,
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Activity {
    activity_hash: u32,
    is_focused_activity: bool,
    is_completed: bool,
    recommended_light: Option<i32>,
    difficulty_tier: Option<u32>,
    #[serde(default)]
    modifier_hashes: Nested<u32>,
    #[serde(default)]
    challenges: Nested<Challenge>,
    #[serde(default)]
    visible_rewards: Nested<VisibleReward>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VisibleReward {
    reward_items: Nested<RewardItem>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RewardItem {
    item_quantity: ItemQuantity,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemQuantity {
    item_hash: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Milestone {
    milestone_hash: u32,
    start_date: Option<String>,
    end_date: Option<String>,
    #[serde(default)]
    activities: Nested<MilestoneActivity>,
    #[serde(default)]
    rewards: Nested<Reward>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MilestoneActivity {
    activity_hash: u32,
    #[serde(default)]
    challenges: Nested<Challenge>,
    #[serde(default)]
    phases: Nested<Phase>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Phase {
    phase_hash: u32,
    complete: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reward {
    reward_category_hash: u32,
    entries: Nested<Entry>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    reward_entry_hash: u32,
    earned: bool,
    redeemed: bool,
}
struct Milestones(Vec<Milestone>);
impl<'de> Deserialize<'de> for Milestones {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Milestones;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded milestones")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                let mut seen = HashSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    let hash: u32 = key
                        .parse()
                        .map_err(|_| de::Error::custom("milestone key"))?;
                    if key != hash.to_string() || !seen.insert(hash) || values.len() == 64 {
                        return Err(de::Error::custom("milestone limit"));
                    }
                    let value: Milestone = map.next_value()?;
                    if value.milestone_hash != hash {
                        return Err(de::Error::custom("milestone mismatch"));
                    }
                    values.push(value);
                }
                values.sort_by_key(|v| v.milestone_hash);
                Ok(Milestones(values))
            }
        }
        d.deserialize_map(V)
    }
}
fn timestamp(raw: Option<String>) -> Result<Option<Timestamp>, AppError> {
    raw.map(|raw| {
        let date = chrono::DateTime::parse_from_rfc3339(&raw).map_err(|_| invalid())?;
        Ok(Timestamp::new(
            u64::try_from(date.timestamp()).map_err(|_| invalid())?,
        ))
    })
    .transpose()
}
/// Assumes DestinyCharacterResponse.progressions/activities component data shape.
pub fn decode_character_weekly(
    bytes: &[u8],
    character: &CharacterId,
) -> Result<CharacterWeeklySnapshot, AppError> {
    if bytes.len() > WEEKLY_RESPONSE_LIMIT {
        return Err(invalid());
    }
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if envelope.code != 1 {
        return Err(invalid());
    }
    let mut activities = Vec::new();
    for a in envelope.response.activities.data.available_activities.0 {
        let mut hashes = Vec::new();
        for item in a
            .visible_rewards
            .0
            .into_iter()
            .flat_map(|r| r.reward_items.0)
        {
            if hashes.len() == 64 {
                return Err(invalid());
            }
            hashes.push(item.item_quantity.item_hash);
        }
        activities.push(AvailableActivity {
            activity_hash: a.activity_hash,
            focused: a.is_focused_activity,
            completed_ever: a.is_completed,
            recommended_light: a.recommended_light,
            difficulty_tier: a.difficulty_tier,
            modifier_hashes: a.modifier_hashes.0,
            challenges: a.challenges.0.into_iter().map(Into::into).collect(),
            visible_reward_item_hashes: hashes,
        });
    }
    let milestones = envelope
        .response
        .progressions
        .data
        .milestones
        .0
        .into_iter()
        .map(|m| {
            Ok(CharacterMilestone {
                milestone_hash: m.milestone_hash,
                start: timestamp(m.start_date)?,
                end: timestamp(m.end_date)?,
                activities: m
                    .activities
                    .0
                    .into_iter()
                    .map(|a| CharacterMilestoneActivity {
                        activity_hash: a.activity_hash,
                        challenges: a.challenges.0.into_iter().map(Into::into).collect(),
                        phases: a
                            .phases
                            .0
                            .into_iter()
                            .map(|p| MilestonePhase {
                                phase_hash: p.phase_hash,
                                complete: p.complete,
                            })
                            .collect(),
                    })
                    .collect(),
                rewards: m
                    .rewards
                    .0
                    .into_iter()
                    .map(|r| MilestoneReward {
                        category_hash: r.reward_category_hash,
                        entries: r
                            .entries
                            .0
                            .into_iter()
                            .map(|e| MilestoneRewardEntry {
                                entry_hash: e.reward_entry_hash,
                                earned: e.earned,
                                redeemed: e.redeemed,
                            })
                            .collect(),
                    })
                    .collect(),
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    let snapshot = CharacterWeeklySnapshot {
        character_id: character.clone(),
        available_activities: activities,
        milestones,
    };
    snapshot.validate()?;
    Ok(snapshot)
}
