//! Live-only, provider-neutral seasonal artifact points.
use crate::{AppError, DestinyMembership};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactSnapshot {
    pub profile: Option<ArtifactProfileState>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactProfileState {
    pub points: u32,
    pub points_cap: u32,
    pub progress_to_next_level: u32,
    pub next_level_at: u32,
}
/// Separate live read port; no persistence or inventory gateway changes.
pub trait ArtifactGateway {
    fn load_artifact(
        &mut self,
        membership: &DestinyMembership,
    ) -> Result<ArtifactSnapshot, AppError>;
}
