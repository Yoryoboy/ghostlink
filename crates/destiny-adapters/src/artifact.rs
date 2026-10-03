//! GetProfile component 104 only; unknown provider fields are ignored.
use destiny_core::*;
use serde::Deserialize;

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
        "https://www.bungie.net/Platform/Destiny2/{}/Profile/{}/?components=104",
        membership.membership_type().value(),
        membership.membership_id().as_str()
    );
    if !is_allowed_artifact_url(&url) {
        return Err(invalid());
    }
    Ok(url)
}
/// Accepts only the canonical origin, decimal identifiers and exact query.
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
        && parts[3] == "?components=104"
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
    point_progression: Option<PointProgression>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PointProgression {
    level: u32,
    level_cap: u32,
    progress_to_next_level: u32,
    next_level_at: u32,
}
/// Decodes artifact point progression; absent/private data is omitted.
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
        .and_then(|a| a.point_progression)
        .map(|p| ArtifactProfileState {
            points: p.level,
            points_cap: p.level_cap,
            progress_to_next_level: p.progress_to_next_level,
            next_level_at: p.next_level_at,
        });
    Ok(ArtifactSnapshot { profile })
}
