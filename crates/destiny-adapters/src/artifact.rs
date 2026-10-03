//! GetProfile components 104/202 only; unknown provider fields are ignored.
use destiny_core::*;
use serde::{
    Deserialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use std::{collections::HashSet, fmt};

pub const ARTIFACT_RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
fn invalid() -> AppError {
    AppError::new(ErrorCategory::ProviderValidation)
}
fn canonical_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes().all(|b| b.is_ascii_digit())
        && (s == "0" || !s.starts_with('0'))
}
/// Canonical identifiers only, before any credential is attached.
pub fn artifact_endpoint(membership: &DestinyMembership) -> Result<String, AppError> {
    let url = format!(
        "https://www.bungie.net/Platform/Destiny2/{}/Profile/{}/?components=104,202",
        membership.membership_type().value(),
        membership.membership_id().as_str()
    );
    if !is_allowed_artifact_url(&url) {
        return Err(invalid());
    }
    Ok(url)
}
/// Accepts only the canonical origin, decimal identifiers and exact ordered query.
pub fn is_allowed_artifact_url(url: &str) -> bool {
    let Some(path) = url.strip_prefix("https://www.bungie.net/Platform/Destiny2/") else {
        return false;
    };
    let parts: Vec<_> = path.split('/').collect();
    parts.len() == 4
        && canonical_id(parts[0])
        && parts[0].parse::<u32>().is_ok_and(|v| v != 0 && v != 254)
        && parts[1] == "Profile"
        && canonical_id(parts[2])
        && parts[3] == "?components=104,202"
}

// Enforce bounds during deserialization, including invisible perks.
struct List<T, const N: usize>(Vec<T>);
impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for List<T, N> {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T, const N: usize>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for V<T, N> {
            type Value = List<T, N>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded artifact list")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element()? {
                    if values.len() == N {
                        return Err(de::Error::custom("artifact list limit"));
                    }
                    values.push(value);
                }
                Ok(List(values))
            }
        }
        d.deserialize_seq(V::<T, N>(std::marker::PhantomData))
    }
}
#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "ErrorCode")]
    code: u32,
    #[serde(rename = "Response")]
    response: Response,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    profile_progression: Option<Component<ProfileProgression>>,
    character_progressions: Option<Component<Characters>>,
}
#[derive(Deserialize)]
struct Component<T> {
    data: Option<T>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileProgression {
    seasonal_artifact: Option<ProfileArtifact>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileArtifact {
    artifact_hash: u32,
    points_acquired: u32,
    power_bonus: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CharacterProgression {
    seasonal_artifact: Option<CharacterArtifact>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CharacterArtifact {
    artifact_hash: u32,
    points_used: u32,
    reset_count: u32,
    tiers: List<Tier, MAX_ARTIFACT_TIERS>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tier {
    tier_hash: u32,
    is_unlocked: bool,
    points_to_unlock: u32,
    items: List<Perk, MAX_ARTIFACT_PERKS>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Perk {
    item_hash: u32,
    is_active: bool,
    is_visible: bool,
}
struct Characters(Vec<(CharacterId, CharacterProgression)>);
impl<'de> Deserialize<'de> for Characters {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Characters;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded character progressions")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                let mut seen = HashSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !canonical_id(&key)
                        || !seen.insert(key.clone())
                        || values.len() == MAX_ARTIFACT_CHARACTERS
                    {
                        return Err(de::Error::custom("artifact character key or limit"));
                    }
                    values.push((CharacterId::new(key), map.next_value()?));
                }
                // Canonical decimal ordering without imposing a u64 width limit.
                values.sort_by(|(a, _), (b, _)| {
                    a.as_str()
                        .len()
                        .cmp(&b.as_str().len())
                        .then_with(|| a.as_str().cmp(b.as_str()))
                });
                Ok(Characters(values))
            }
        }
        d.deserialize_map(V)
    }
}
/// Decodes profile and per-character seasonal artifacts; absent/private data is omitted.
pub fn decode_artifact(bytes: &[u8]) -> Result<ArtifactSnapshot, AppError> {
    if bytes.len() > ARTIFACT_RESPONSE_LIMIT {
        return Err(invalid());
    }
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if envelope.code != 1 {
        return Err(invalid());
    }
    let profile = envelope
        .response
        .profile_progression
        .and_then(|c| c.data)
        .and_then(|p| p.seasonal_artifact)
        .map(|a| ArtifactProfileState {
            artifact_hash: ItemHash::new(i64::from(a.artifact_hash)),
            points_acquired: a.points_acquired,
            power_bonus: a.power_bonus,
        });
    let characters = envelope
        .response
        .character_progressions
        .and_then(|c| c.data)
        .map(|c| c.0)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(character_id, p)| {
            p.seasonal_artifact.map(|a| ArtifactCharacterState {
                character_id,
                artifact_hash: ItemHash::new(i64::from(a.artifact_hash)),
                points_used: a.points_used,
                reset_count: a.reset_count,
                tiers: a
                    .tiers
                    .0
                    .into_iter()
                    .map(|t| ArtifactTier {
                        tier_hash: t.tier_hash,
                        is_unlocked: t.is_unlocked,
                        points_to_unlock: t.points_to_unlock,
                        perks: t
                            .items
                            .0
                            .into_iter()
                            .filter(|p| p.is_visible)
                            .map(|p| ArtifactPerk {
                                item_hash: ItemHash::new(i64::from(p.item_hash)),
                                active: p.is_active,
                            })
                            .collect(),
                    })
                    .collect(),
            })
        })
        .collect();
    let snapshot = ArtifactSnapshot {
        profile,
        characters,
    };
    snapshot.validate()?;
    Ok(snapshot)
}
