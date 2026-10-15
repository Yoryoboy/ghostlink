//! Isolated community-data synchronization after successful Manifest sync.
use destiny_core::{AppError, ClarityGateway, ClarityRepository, Timestamp};

/// Request-local cache of catalog reads, including misses. Matching stays parent-specific.
#[derive(Default)]
pub(crate) struct ClarityCache {
    hashes: std::collections::HashMap<u32, Option<destiny_core::ClarityInsight>>,
    names: std::collections::HashMap<String, Vec<destiny_core::ClarityInsight>>,
}

impl ClarityCache {
    pub(crate) fn resolve(
        &mut self,
        repository: &impl destiny_core::ports::WorldDefinitionRepository,
        plug: &destiny_core::ItemDefinition,
        parent: Option<destiny_core::ItemHash>,
    ) -> Result<(Option<String>, Option<String>), AppError> {
        let Ok(hash) = u32::try_from(plug.hash.value()) else {
            return Ok((None, None));
        };
        if let std::collections::hash_map::Entry::Vacant(entry) = self.hashes.entry(hash) {
            entry.insert(repository.lookup_clarity_by_hash(hash)?);
        }
        let entries = if let Some(exact) = &self.hashes[&hash] {
            std::slice::from_ref(exact)
        } else {
            if !self.names.contains_key(&plug.name) {
                self.names.insert(
                    plug.name.clone(),
                    repository.lookup_clarity_by_name(&plug.name)?,
                );
            }
            &self.names[&plug.name]
        };
        let matched = destiny_core::clarity::match_clarity(
            hash,
            &plug.name,
            plug.item_type.as_deref(),
            plug.plug_category
                .as_deref()
                .map(destiny_core::domain::classify_plug),
            parent.and_then(|hash| u32::try_from(hash.value()).ok()),
            entries,
        );
        Ok(match matched {
            Some(matched) => (
                Some(matched.text),
                matched.variant.then(|| "variant".into()),
            ),
            None => (None, None),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaritySyncReport {
    pub status: &'static str,
    pub version: Option<String>,
    pub entries_stored: usize,
    pub entries_skipped: usize,
}
impl ClaritySyncReport {
    pub fn unavailable() -> Self {
        Self {
            status: "unavailable",
            version: None,
            entries_stored: 0,
            entries_skipped: 0,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestAndClarityReport {
    pub manifest: crate::ManifestSyncReport,
    pub clarity: ClaritySyncReport,
    /// `unavailable` until a caller runs the wish-list step.
    pub wishlists: crate::wishlist::WishlistSyncReport,
}

/// Errors are converted into a report, never returned to the Manifest caller.
pub fn sync_clarity<G: ClarityGateway, R: ClarityRepository>(
    gateway: &mut G,
    repository: &mut R,
    checked_at: Timestamp,
) -> ClaritySyncReport {
    sync(gateway, repository, checked_at).unwrap_or_else(|_| ClaritySyncReport::unavailable())
}
fn sync<G: ClarityGateway, R: ClarityRepository>(
    gateway: &mut G,
    repository: &mut R,
    checked_at: Timestamp,
) -> Result<ClaritySyncReport, AppError> {
    let version = gateway.version()?;
    if let Some(metadata) = repository.clarity_metadata()?
        && metadata.version == version
    {
        repository.mark_clarity_checked(checked_at)?;
        return Ok(ClaritySyncReport {
            status: "unchanged",
            version: Some(version),
            entries_stored: metadata.entries,
            entries_skipped: 0,
        });
    }
    let batch = gateway.descriptions()?;
    repository.publish_clarity(&version, &batch.entries, checked_at)?;
    Ok(ClaritySyncReport {
        status: "updated",
        version: Some(version),
        entries_stored: batch.entries.len(),
        entries_skipped: batch.skipped,
    })
}

/// Shared runtime seam: Manifest finishes first, even before Clarity construction.
pub fn sync_manifest_and_clarity<M, R, G>(
    service: &mut crate::ManifestService<M, R>,
    acquire: impl FnOnce() -> Result<G, AppError>,
    checked_at: Timestamp,
) -> Result<ManifestAndClarityReport, AppError>
where
    M: destiny_core::ManifestGateway,
    R: destiny_core::ports::DefinitionCatalogRepository
        + destiny_core::ports::WorldDefinitionRepository
        + destiny_core::StatDefinitionRepository
        + ClarityRepository,
    G: ClarityGateway,
{
    let manifest = service.sync(checked_at)?;
    let clarity = match acquire() {
        Ok(mut gateway) => sync_clarity(&mut gateway, service.repository_mut(), checked_at),
        Err(_) => ClaritySyncReport::unavailable(),
    };
    Ok(ManifestAndClarityReport {
        manifest,
        clarity,
        wishlists: crate::wishlist::WishlistSyncReport::unavailable(),
    })
}
