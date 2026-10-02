//! Field-limited Bungie public milestone decoding. Provider text never escapes.
use destiny_core::{
    AppError, PublicMilestone, PublicMilestoneActivity, PublicMilestonesSnapshot, Timestamp,
};
use serde::{
    Deserialize,
    de::{self, MapAccess, Visitor},
};
use std::{collections::HashSet, fmt};

/// Formats a public reset instant in UTC without exposing provider payloads.
/// Unrepresentable domain timestamps are returned as `None`.
pub fn format_reset_timestamp(timestamp: Timestamp, human: bool) -> Option<String> {
    let seconds = i64::try_from(timestamp.seconds()).ok()?;
    let date = chrono::DateTime::from_timestamp(seconds, 0)?;
    Some(if human {
        date.format("%Y-%m-%d %H:%MZ").to_string()
    } else {
        date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    })
}

fn invalid() -> AppError {
    AppError::new(destiny_core::ErrorCategory::ProviderValidation)
}
#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "ErrorCode")]
    error_code: u64,
    #[serde(rename = "ErrorStatus")]
    error_status: String,
    #[serde(rename = "Response", default)]
    response: Option<Milestones>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    milestone_hash: u32,
    order: u32,
    start_date: Option<String>,
    end_date: Option<String>,
    #[serde(default)]
    activities: Vec<Activity>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Activity {
    activity_hash: u32,
    #[serde(default)]
    modifier_hashes: Vec<u32>,
}
struct Milestones(Vec<PublicMilestone>);
impl<'de> Deserialize<'de> for Milestones {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MilestoneVisitor;
        impl<'de> Visitor<'de> for MilestoneVisitor {
            type Value = Milestones;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("public milestones")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut seen = HashSet::new();
                let mut milestones = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    let hash = key
                        .parse::<u32>()
                        .map_err(|_| de::Error::custom("invalid milestone"))?;
                    if key != hash.to_string() || !seen.insert(hash) || seen.len() > 64 {
                        return Err(de::Error::custom("invalid milestone"));
                    }
                    let record = map.next_value::<Record>()?;
                    if record.milestone_hash != hash {
                        return Err(de::Error::custom("invalid milestone"));
                    }
                    let timestamp = |value: Option<String>| -> Result<Option<Timestamp>, M::Error> {
                        value
                            .map(|raw| {
                                let parsed = chrono::DateTime::parse_from_rfc3339(&raw)
                                    .map_err(|_| de::Error::custom("invalid timestamp"))?;
                                let seconds = u64::try_from(parsed.timestamp())
                                    .map_err(|_| de::Error::custom("invalid timestamp"))?;
                                Ok(Timestamp::new(seconds))
                            })
                            .transpose()
                    };
                    milestones.push(PublicMilestone {
                        milestone_hash: hash,
                        order: record.order,
                        start: timestamp(record.start_date)?,
                        end: timestamp(record.end_date)?,
                        activities: record
                            .activities
                            .into_iter()
                            .map(|activity| PublicMilestoneActivity {
                                activity_hash: activity.activity_hash,
                                modifier_hashes: activity.modifier_hashes,
                            })
                            .collect(),
                    });
                }
                // Canonical ordering independent of provider object order.
                milestones.sort_by_key(|milestone| milestone.milestone_hash);
                Ok(Milestones(milestones))
            }
        }
        deserializer.deserialize_map(MilestoneVisitor)
    }
}
/// Decodes a bounded envelope and retains only the public domain projection.
pub fn decode_public_milestones(bytes: &[u8]) -> Result<PublicMilestonesSnapshot, AppError> {
    if bytes.len() > 1024 * 1024 {
        return Err(invalid());
    }
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if envelope.error_code != 1 || envelope.error_status != "Success" {
        return Err(invalid());
    }
    let snapshot = PublicMilestonesSnapshot {
        milestones: envelope.response.ok_or_else(invalid)?.0,
    };
    snapshot.validate()?;
    Ok(snapshot)
}
