//! Manifest acquisition validation, normalization, and atomic publication.
//!
//! The service is provider- and transport-neutral. It consumes untrusted
//! candidates from the core-owned `ManifestGateway`, validates and normalizes
//! them into core `ItemDefinition` records, and publishes one complete
//! `ManifestDefinitions` set through the core-owned `DefinitionRepository`. It
//! never retains or surfaces raw payloads, upstream messages, or transport
//! details. The caller supplies a `checked_at` timestamp explicitly: the
//! service sets the Manifest source marker itself and delegates durable
//! revision allocation to the repository.

use destiny_core::{
    AppError, DefinitionReplacementInput, DefinitionRepository, DefinitionVersion, ErrorCategory,
    ItemDefinition, ManifestAcquisition, ManifestDefinitions, ManifestGateway, SourceMarker,
    SyncRevision, Timestamp,
};
use std::collections::HashSet;

/// Maximum accepted length of a definition version identifier, in characters.
pub const MAX_VERSION_LENGTH: usize = 128;

/// Safety ceiling for the number of candidates in one acquisition.
///
/// This is a defensive boundary, not a completeness claim. It sits several times
/// above any plausible provider item-definition catalog so a legitimate catalog
/// is never silently truncated, while still bounding memory. Validating the real
/// catalog fixture against this ceiling is DMF-004 scope.
pub const MAX_DEFINITION_COUNT: usize = 100_000;

/// Maximum accepted length of a presentation or classification string.
pub const MAX_TEXT_LENGTH: usize = 256;

/// Maximum accepted number of tags on a single definition.
pub const MAX_TAG_COUNT: usize = 32;

/// Maximum accepted length of a single normalized tag, in characters.
pub const MAX_TAG_LENGTH: usize = 64;

/// A minimal, non-owning summary of one successful publication.
///
/// The service returns this instead of the full definition set so a successful
/// publication never clones the complete catalog. Callers read the published
/// definitions back through the repository. The committed `revision` comes from
/// the repository, which owns retention and advancement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicationOutcome {
    pub version: DefinitionVersion,
    pub definition_count: usize,
    pub skipped_count: usize,
    pub revision: SyncRevision,
}

/// Version-only labels and counts suitable for presentation; never catalog data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManifestSyncReport {
    Unchanged {
        version: DefinitionVersion,
        record_count: usize,
        /// Milestone, activity, modifier, objective, activity-type counts.
        world_counts: [usize; 5],
        stat_count: usize,
    },
    Updated {
        version: DefinitionVersion,
        record_count: usize,
        skipped: usize,
        world_counts: [usize; 5],
        world_skipped: [usize; 5],
        stat_count: usize,
        stat_skipped: usize,
    },
}

/// Acquires, validates, normalizes, and atomically publishes Manifest definitions.
///
/// Publication is all-or-nothing: a gateway error, an invalid candidate, or a
/// repository rejection leaves the previously visible repository state
/// untouched. A successful publication replaces the complete set.
pub struct ManifestService<G, R> {
    gateway: G,
    repository: R,
}

impl<G, R> ManifestService<G, R>
where
    G: ManifestGateway,
    R: DefinitionRepository,
{
    pub fn new(gateway: G, repository: R) -> Self {
        Self {
            gateway,
            repository,
        }
    }

    /// Borrows the gateway for acquisition diagnostics in deterministic tests.
    pub fn gateway(&self) -> &G {
        &self.gateway
    }

    /// Borrows the repository so callers can read the visible state.
    pub fn repository(&self) -> &R {
        &self.repository
    }

    /// Mutably borrows the repository for test or composition setup.
    pub fn repository_mut(&mut self) -> &mut R {
        &mut self.repository
    }

    /// Acquires one candidate batch and publishes its normalized complete set.
    ///
    /// The caller supplies the explicit `checked_at` timestamp; the service
    /// never reads an ambient clock. It sets the Manifest source marker and
    /// publishes through [`DefinitionReplacementInput`]. Returns a minimal
    /// [`PublicationOutcome`] carrying the committed revision rather than the
    /// full set, so the complete catalog is never cloned during publication. A
    /// gateway failure or any validation failure returns a sanitized
    /// [`AppError`] and performs no repository mutation.
    pub fn acquire_and_publish(
        &mut self,
        checked_at: Timestamp,
    ) -> Result<PublicationOutcome, AppError> {
        let acquisition = self.gateway.load_definitions()?;
        self.publish(acquisition, checked_at)
    }

    fn publish(
        &mut self,
        acquisition: ManifestAcquisition,
        checked_at: Timestamp,
    ) -> Result<PublicationOutcome, AppError> {
        let (definitions, skipped_count) = normalize_with_count(acquisition)?;
        let version = definitions.version.clone();
        let definition_count = definitions.definitions.len();
        let metadata = self
            .repository
            .replace_definitions(DefinitionReplacementInput {
                definitions,
                checked_at,
                source: SourceMarker::Manifest,
            })?;
        Ok(PublicationOutcome {
            version,
            definition_count,
            skipped_count,
            revision: metadata.revision,
        })
    }
}

impl<G, R> ManifestService<G, R>
where
    G: ManifestGateway,
    R: destiny_core::ports::DefinitionCatalogRepository
        + destiny_core::ports::WorldDefinitionRepository
        + destiny_core::ports::StatDefinitionRepository,
{
    /// Fetches the small index first. An exact version match with all world
    /// and stat catalogs populated is a pure read. Missing catalogs (including
    /// a migrated item-only store) require a complete atomic publication.
    /// Storage failures propagate rather than implying an empty catalog.
    pub fn sync(&mut self, checked_at: Timestamp) -> Result<ManifestSyncReport, AppError> {
        let index = self.gateway.fetch_index()?;
        let version = normalize_version(index.version.as_str())?;
        if let Some(metadata) = self.repository.current_metadata()?
            && metadata.definition_version == version
        {
            let world_counts = self.repository.world_definition_counts()?;
            let stat_count = self.repository.stat_definition_count()?;
            if world_counts.iter().all(|count| *count > 0) && stat_count > 0 {
                return Ok(ManifestSyncReport::Unchanged {
                    version,
                    record_count: self.repository.definition_count()?,
                    world_counts,
                    stat_count,
                });
            }
        }
        let acquisition = self.gateway.load_component(&index)?;
        if acquisition.version != version {
            return Err(invalid());
        }
        let (definitions, skipped) = normalize_with_count(acquisition)?;
        let world = self.gateway.load_world_components(&index)?;
        world.validate_for(&version)?;
        let world_counts = world.counts();
        let world_skipped = world.skipped_counts();
        let stat_count = world.stats.definitions.len();
        let stat_skipped = world.stats.skipped_count;
        let record_count = definitions.definitions.len();
        self.repository.publish_manifest(
            DefinitionReplacementInput {
                definitions,
                checked_at,
                source: SourceMarker::Manifest,
            },
            world,
        )?;
        Ok(ManifestSyncReport::Updated {
            version,
            record_count,
            skipped,
            world_counts,
            world_skipped,
            stat_count,
            stat_skipped,
        })
    }
}

/// Validates and normalizes one untrusted acquisition into a publishable set.
///
/// Casing is split by role rather than folded globally:
///
/// - Presentation and classification values keep their authoritative casing.
///   `ItemDefinition.name`, `item_type`, `tier`, and `element` are trimmed and
///   have internal whitespace runs collapsed, but their case is never rewritten,
///   because the display and classification strings belong to the provider and
///   the user.
/// - The opaque provider `version` is a single exact identifier. It must be
///   non-empty, bounded, and free of any whitespace or control character; it is
///   compared exactly and never rewritten.
/// - Only dedicated search-only values (`tags`) are lowercased, because their
///   sole role is case-insensitive search.
///
/// Other rules:
///
/// - Prohibited control characters are any Unicode control characters other
///   than the whitespace characters normalization collapses.
/// - The length bound is re-checked after every transformation that can change
///   the character count.
/// - Optional fields (`item_type`, `tier`, `element`) that normalize to empty
///   become absent; blank tags are dropped.
/// - Tags are normalized, sorted, and deduplicated; output records are ordered
///   by definition hash.
///
/// Validation rejects a blank, whitespace-padded, or oversized version; an empty
/// or oversized candidate set; a candidate version that disagrees exactly with
/// the batch version; duplicate definition hashes; missing or blank required
/// search fields (blank names are skipped); prohibited control characters; and oversized fields. Every
/// rejection returns [`ErrorCategory::ProviderValidation`].
pub fn normalize_manifest(
    acquisition: ManifestAcquisition,
) -> Result<ManifestDefinitions, AppError> {
    normalize_with_count(acquisition).map(|(definitions, _)| definitions)
}

fn normalize_with_count(
    acquisition: ManifestAcquisition,
) -> Result<(ManifestDefinitions, usize), AppError> {
    let mut skipped_count = acquisition.skipped_count;
    let version = normalize_version(acquisition.version.as_str())?;
    let candidates = acquisition.definitions;
    if candidates.is_empty() || candidates.len() > MAX_DEFINITION_COUNT {
        return Err(invalid());
    }

    let mut seen = HashSet::with_capacity(candidates.len());
    let mut definitions = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if normalize_version(candidate.version.as_str())? != version {
            return Err(invalid());
        }
        if !seen.insert(candidate.hash) {
            return Err(invalid());
        }
        let Some(name) = normalize_required_presentation(&candidate.name)? else {
            skipped_count = skipped_count.checked_add(1).ok_or_else(invalid)?;
            continue;
        };
        definitions.push(ItemDefinition {
            hash: candidate.hash,
            bucket_hash: candidate.bucket_hash,
            name,
            item_type: normalize_optional_presentation(candidate.item_type.as_deref())?,
            tier: normalize_optional_presentation(candidate.tier.as_deref())?,
            element: match candidate.damage_type {
                Some(damage) => Some(damage.as_str().to_owned()),
                None => normalize_optional_presentation(candidate.element.as_deref())?,
            },
            slot: candidate.slot,
            class_type: candidate.class_type,
            plug_category: normalize_optional_presentation(candidate.plug_category.as_deref())?,
            tags: normalize_tags(&candidate.tags)?,
            version: version.clone(),
        });
    }
    if definitions.is_empty() {
        return Err(invalid());
    }
    definitions.sort_by_key(|definition| definition.hash);
    Ok((
        ManifestDefinitions {
            version,
            definitions,
        },
        skipped_count,
    ))
}

fn invalid() -> AppError {
    AppError::new(ErrorCategory::ProviderValidation)
}

/// Rejects prohibited control characters and raw input longer than the bound.
fn reject_prohibited_text(value: &str, max_length: usize) -> Result<(), AppError> {
    let has_prohibited_control = value
        .chars()
        .any(|character| character.is_control() && !character.is_whitespace());
    if has_prohibited_control || value.chars().count() > max_length {
        return Err(invalid());
    }
    Ok(())
}

/// Re-checks the bound after a transformation that can change character count.
fn reject_normalized_length(value: &str, max_length: usize) -> Result<(), AppError> {
    if value.chars().count() > max_length {
        return Err(invalid());
    }
    Ok(())
}

/// Collapses whitespace runs without changing case.
fn collapse_whitespace(value: &str) -> String {
    let mut collapsed = String::with_capacity(value.len());
    for word in value.split_whitespace() {
        if !collapsed.is_empty() {
            collapsed.push(' ');
        }
        collapsed.push_str(word);
    }
    collapsed
}

/// Trims and collapses whitespace while preserving authoritative casing.
fn normalize_presentation(value: &str, max_length: usize) -> Result<String, AppError> {
    reject_prohibited_text(value, max_length)?;
    let normalized = collapse_whitespace(value);
    reject_normalized_length(&normalized, max_length)?;
    Ok(normalized)
}

/// Canonicalizes a search-only value: whitespace collapse plus lowercase.
fn normalize_search(value: &str, max_length: usize) -> Result<String, AppError> {
    reject_prohibited_text(value, max_length)?;
    let normalized = collapse_whitespace(value).to_lowercase();
    reject_normalized_length(&normalized, max_length)?;
    Ok(normalized)
}

/// Validates a non-empty bounded exact identifier without rewriting it.
fn normalize_version(value: &str) -> Result<DefinitionVersion, AppError> {
    let has_invalid_character = value
        .chars()
        .any(|character| character.is_control() || character.is_whitespace());
    if value.is_empty() || value.chars().count() > MAX_VERSION_LENGTH || has_invalid_character {
        return Err(invalid());
    }
    Ok(DefinitionVersion::new(value.to_owned()))
}

fn normalize_required_presentation(value: &str) -> Result<Option<String>, AppError> {
    let normalized = normalize_presentation(value, MAX_TEXT_LENGTH)?;
    Ok((!normalized.is_empty()).then_some(normalized))
}

fn normalize_optional_presentation(value: Option<&str>) -> Result<Option<String>, AppError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let normalized = normalize_presentation(value, MAX_TEXT_LENGTH)?;
    Ok(if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    })
}

fn normalize_tags(tags: &[String]) -> Result<Vec<String>, AppError> {
    if tags.len() > MAX_TAG_COUNT {
        return Err(invalid());
    }
    let mut normalized = Vec::with_capacity(tags.len());
    for tag in tags {
        let value = normalize_search(tag, MAX_TAG_LENGTH)?;
        if !value.is_empty() {
            normalized.push(value);
        }
    }
    normalized.sort_unstable();
    normalized.dedup();
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::{BucketHash, ItemHash, ManifestDefinitionCandidate};

    const VERSION: &str = "manifest-2026-09-21";

    #[test]
    fn plug_category_normalization_preserves_identifier_and_bounds_input() {
        let mut plug = candidate(1, "Trait");
        plug.plug_category = Some(" frames ".into());
        let published = normalize_manifest(acquisition(VERSION, vec![plug.clone()])).unwrap();
        assert_eq!(
            published.definitions[0].plug_category.as_deref(),
            Some("frames")
        );
        plug.plug_category = Some(" ".into());
        let published = normalize_manifest(acquisition(VERSION, vec![plug.clone()])).unwrap();
        assert_eq!(published.definitions[0].plug_category, None);
        for invalid in ["x".repeat(257), "frames\0".into()] {
            plug.plug_category = Some(invalid);
            assert!(normalize_manifest(acquisition(VERSION, vec![plug.clone()])).is_err());
        }
    }

    fn candidate(hash: i64, name: &str) -> ManifestDefinitionCandidate {
        ManifestDefinitionCandidate {
            hash: ItemHash::new(hash),
            bucket_hash: None,
            name: name.into(),
            item_type: None,
            tier: None,
            element: None,
            damage_type: None,
            slot: None,
            class_type: None,
            plug_category: None,
            tags: Vec::new(),
            version: DefinitionVersion::new(VERSION),
        }
    }

    fn candidate_versioned(hash: i64, name: &str, version: &str) -> ManifestDefinitionCandidate {
        ManifestDefinitionCandidate {
            version: DefinitionVersion::new(version),
            ..candidate(hash, name)
        }
    }

    fn acquisition(
        version: &str,
        definitions: Vec<ManifestDefinitionCandidate>,
    ) -> ManifestAcquisition {
        ManifestAcquisition {
            version: DefinitionVersion::new(version),
            definitions,
            skipped_count: 0,
        }
    }

    #[test]
    fn equal_semantic_acquisitions_normalize_identically() {
        let noisy = acquisition(
            VERSION,
            vec![
                ManifestDefinitionCandidate {
                    hash: ItemHash::new(2),
                    bucket_hash: Some(BucketHash::new(11)),
                    name: "  Ri   FLE  ".into(),
                    item_type: Some("  WEAPON ".into()),
                    tier: Some(" Legendary ".into()),
                    element: Some("  SOLAR ".into()),
                    damage_type: None,
                    slot: None,
                    class_type: None,
                    plug_category: None,
                    tags: vec!["Tag-B".into(), "  tag-a ".into(), "TAG-A".into()],
                    version: DefinitionVersion::new(VERSION),
                },
                candidate(1, "Alpha"),
            ],
        );
        let tidy = acquisition(
            VERSION,
            vec![
                candidate(1, "Alpha"),
                ManifestDefinitionCandidate {
                    hash: ItemHash::new(2),
                    bucket_hash: Some(BucketHash::new(11)),
                    name: "Ri FLE".into(),
                    item_type: Some("WEAPON".into()),
                    tier: Some("Legendary".into()),
                    element: Some("SOLAR".into()),
                    damage_type: None,
                    slot: None,
                    class_type: None,
                    plug_category: None,
                    tags: vec!["tag-a".into(), "tag-b".into()],
                    version: DefinitionVersion::new(VERSION),
                },
            ],
        );

        let first = normalize_manifest(noisy).unwrap();
        let second = normalize_manifest(tidy).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.version.as_str(), VERSION);
        assert_eq!(first.definitions[0].hash, ItemHash::new(1));
        assert_eq!(first.definitions[0].name, "Alpha");
        assert_eq!(first.definitions[1].name, "Ri FLE");
        assert_eq!(first.definitions[1].item_type.as_deref(), Some("WEAPON"));
        assert_eq!(first.definitions[1].tier.as_deref(), Some("Legendary"));
        assert_eq!(first.definitions[1].element.as_deref(), Some("SOLAR"));
        assert_eq!(
            first.definitions[1].tags,
            vec!["tag-a".to_string(), "tag-b".to_string()]
        );
    }

    #[test]
    fn presentation_and_version_casing_are_preserved_not_folded() {
        let lower = normalize_manifest(acquisition(VERSION, vec![candidate(1, "alpha")])).unwrap();
        let upper = normalize_manifest(acquisition(VERSION, vec![candidate(1, "Alpha")])).unwrap();

        assert_ne!(lower, upper);
        assert_eq!(lower.definitions[0].name, "alpha");
        assert_eq!(upper.definitions[0].name, "Alpha");

        let mut classified = candidate(2, "Gamma");
        classified.item_type = Some("Weapon".into());
        classified.tier = Some("Exotic".into());
        classified.element = Some("Solar".into());

        let published = normalize_manifest(acquisition(VERSION, vec![classified])).unwrap();
        assert_eq!(
            published.definitions[0].item_type.as_deref(),
            Some("Weapon")
        );
        assert_eq!(published.definitions[0].tier.as_deref(), Some("Exotic"));
        assert_eq!(published.definitions[0].element.as_deref(), Some("Solar"));
    }

    #[test]
    fn versions_must_be_exact_whitespace_free_identifiers() {
        let padded = " v1 ".to_string();
        let internal = "v 1".to_string();
        let oversized = "v".repeat(MAX_VERSION_LENGTH + 1);

        let cases = [
            acquisition("", vec![candidate(1, "Alpha")]),
            acquisition("   ", vec![candidate(1, "Alpha")]),
            acquisition(&padded, vec![candidate(1, "Alpha")]),
            acquisition(&internal, vec![candidate(1, "Alpha")]),
            acquisition(&oversized, vec![candidate(1, "Alpha")]),
            acquisition("v1", vec![candidate_versioned(1, "Alpha", "V1")]),
        ];

        for case in cases {
            assert_eq!(
                normalize_manifest(case).unwrap_err(),
                AppError::new(ErrorCategory::ProviderValidation)
            );
        }

        let published = normalize_manifest(acquisition(
            "Manifest-V1",
            vec![candidate_versioned(1, "Alpha", "Manifest-V1")],
        ))
        .unwrap();
        assert_eq!(published.version.as_str(), "Manifest-V1");
    }

    #[test]
    fn blank_required_names_are_skipped_when_records_remain() {
        let published = normalize_manifest(acquisition(
            VERSION,
            vec![candidate(1, " \t\n"), candidate(2, "Kept")],
        ))
        .expect("remaining record");
        assert_eq!(published.definitions.len(), 1);
        assert_eq!(published.definitions[0].hash.value(), 2);
        let mut batch = acquisition(VERSION, vec![candidate(1, " "), candidate(2, "Kept")]);
        batch.skipped_count = 2;
        assert_eq!(normalize_with_count(batch).unwrap().1, 3);
    }

    #[test]
    fn blank_optional_fields_normalize_to_absent() {
        let mut item = candidate(9, "Gamma");
        item.item_type = Some("   ".into());
        item.tier = Some(String::new());
        item.element = Some("\t \n".into());
        item.tags = vec!["  ".into(), " Tag ".into()];

        let published = normalize_manifest(acquisition(VERSION, vec![item])).unwrap();
        let definition = &published.definitions[0];

        assert_eq!(definition.item_type, None);
        assert_eq!(definition.tier, None);
        assert_eq!(definition.element, None);
        assert_eq!(definition.tags, vec!["tag".to_string()]);
    }

    #[test]
    fn definitions_are_ordered_by_hash() {
        let published = normalize_manifest(acquisition(
            VERSION,
            vec![
                candidate(30, "Third"),
                candidate(10, "First"),
                candidate(20, "Second"),
            ],
        ))
        .unwrap();

        let hashes: Vec<i64> = published
            .definitions
            .iter()
            .map(|definition| definition.hash.value())
            .collect();
        assert_eq!(hashes, vec![10, 20, 30]);
    }

    #[test]
    fn manifest_records_expose_only_definition_identity() {
        // Exhaustive destructuring is a type-level guarantee. If a runtime
        // instance identifier, a provider metadata flag, or any other
        // non-definition field were added to the candidate or to the normalized
        // record, this test would stop compiling.
        let ManifestDefinitionCandidate {
            hash,
            bucket_hash,
            name,
            item_type,
            tier,
            element,
            damage_type: _,
            slot: _,
            class_type: _,
            plug_category: _,
            tags,
            version,
        } = candidate(1, "Alpha");

        assert_eq!(hash, ItemHash::new(1));
        assert_eq!(bucket_hash, None);
        assert_eq!(name, "Alpha");
        assert_eq!(item_type, None);
        assert_eq!(tier, None);
        assert_eq!(element, None);
        assert!(tags.is_empty());
        assert_eq!(version.as_str(), VERSION);

        let published =
            normalize_manifest(acquisition(VERSION, vec![candidate(1, "Alpha")])).unwrap();
        let ItemDefinition {
            hash,
            bucket_hash,
            name,
            item_type,
            tier,
            element,
            tags,
            version,
            slot: _,
            class_type: _,
            plug_category: _,
        } = published.definitions.into_iter().next().unwrap();

        assert_eq!(hash, ItemHash::new(1));
        assert_eq!(bucket_hash, None);
        assert_eq!(name, "Alpha");
        assert_eq!(item_type, None);
        assert_eq!(tier, None);
        assert_eq!(element, None);
        assert!(tags.is_empty());
        assert_eq!(version.as_str(), VERSION);
    }

    #[test]
    fn malformed_and_incomplete_candidates_are_rejected() {
        let mut blank_name = candidate(1, "Alpha");
        blank_name.name = "   ".into();

        let mut control_name = candidate(2, "Alpha");
        control_name.name = format!("bad{}name", '\u{7}');

        let mut oversized_name = candidate(3, "Alpha");
        oversized_name.name = "a".repeat(MAX_TEXT_LENGTH + 1);

        let mut too_many_tags = candidate(4, "Alpha");
        too_many_tags.tags = (0..=MAX_TAG_COUNT)
            .map(|index| format!("tag-{index}"))
            .collect();

        let mut oversized_tag = candidate(5, "Alpha");
        oversized_tag.tags = vec!["t".repeat(MAX_TAG_LENGTH + 1)];

        let cases = [
            acquisition(VERSION, Vec::new()),
            acquisition(VERSION, vec![blank_name]),
            acquisition(VERSION, vec![control_name]),
            acquisition(VERSION, vec![oversized_name]),
            acquisition(
                VERSION,
                vec![candidate(1, "Alpha"), candidate(1, "Alpha duplicate")],
            ),
            acquisition(VERSION, vec![too_many_tags]),
            acquisition(VERSION, vec![oversized_tag]),
        ];

        for case in cases {
            assert_eq!(
                normalize_manifest(case).unwrap_err(),
                AppError::new(ErrorCategory::ProviderValidation)
            );
        }
    }

    #[test]
    fn candidate_count_boundary_is_enforced() {
        let at_bound: Vec<_> = (0..MAX_DEFINITION_COUNT as i64)
            .map(|hash| candidate(hash, "Bound"))
            .collect();
        let published = normalize_manifest(acquisition(VERSION, at_bound)).unwrap();
        assert_eq!(published.definitions.len(), MAX_DEFINITION_COUNT);

        let over_bound: Vec<_> = (0..=MAX_DEFINITION_COUNT as i64)
            .map(|hash| candidate(hash, "Bound"))
            .collect();
        assert_eq!(
            normalize_manifest(acquisition(VERSION, over_bound)).unwrap_err(),
            AppError::new(ErrorCategory::ProviderValidation)
        );
    }

    #[test]
    fn normalized_length_is_rechecked_after_case_folding() {
        // U+0130 lowercases to two code points, so a raw tag that already fits
        // the bound grows past it once folded and must be rejected.
        let expanded = "\u{0130}".repeat(MAX_TAG_LENGTH);
        assert_eq!(expanded.chars().count(), MAX_TAG_LENGTH);

        let mut item = candidate(1, "Alpha");
        item.tags = vec![expanded];

        assert_eq!(
            normalize_manifest(acquisition(VERSION, vec![item])).unwrap_err(),
            AppError::new(ErrorCategory::ProviderValidation)
        );
    }

    #[test]
    fn failures_never_surface_raw_payloads_or_upstream_messages() {
        const SENTINEL: &str = "raw-provider-payload-sentinel";
        let mut tainted = candidate(1, "Alpha");
        tainted.tags = vec![format!("{SENTINEL}\u{7}")];

        let error = normalize_manifest(acquisition(VERSION, vec![tainted]))
            .expect_err("prohibited control characters must be rejected");

        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
        for rendered in [error.to_string(), format!("{error:?}")] {
            assert!(
                !rendered.contains(SENTINEL),
                "leaked raw payload: {rendered}"
            );
        }
    }

    #[cfg(feature = "test-support")]
    mod behavioral {
        use super::{VERSION, acquisition, candidate};
        use crate::manifest::ManifestService;
        use crate::test_support::{InMemoryDefinitionRepository, QueuedManifestGateway};
        use destiny_core::{
            AppError, Dataset, DefinitionRepository, DefinitionVersion, ErrorCategory,
            ItemDefinition, ItemHash, ManifestAcquisition, Resolution, SourceMarker, Timestamp,
        };

        const CHECKED_AT: Timestamp = Timestamp::new(1_000);

        fn item_definition(hash: i64, name: &str, version: &str) -> ItemDefinition {
            ItemDefinition {
                hash: ItemHash::new(hash),
                bucket_hash: None,
                name: name.into(),
                item_type: None,
                tier: None,
                element: None,
                slot: None,
                class_type: None,
                plug_category: None,
                tags: Vec::new(),
                version: DefinitionVersion::new(version),
            }
        }

        fn seeded_repository() -> InMemoryDefinitionRepository {
            InMemoryDefinitionRepository::new(
                DefinitionVersion::new("manifest-prior"),
                vec![item_definition(7, "prior", "manifest-prior")],
            )
        }

        fn assert_prior_state(repository: &InMemoryDefinitionRepository) {
            assert_eq!(
                repository.current_definition_version().unwrap().as_str(),
                "manifest-prior"
            );
            assert_eq!(repository.definition_count(), 1);
            assert_eq!(
                repository.lookup_definition(ItemHash::new(7)).unwrap(),
                Resolution::Unique(item_definition(7, "prior", "manifest-prior"))
            );
        }

        fn queued_service(
            outcomes: impl IntoIterator<Item = Result<ManifestAcquisition, AppError>>,
            repository: InMemoryDefinitionRepository,
        ) -> ManifestService<QueuedManifestGateway, InMemoryDefinitionRepository> {
            let mut gateway = QueuedManifestGateway::new();
            for outcome in outcomes {
                gateway.queue(outcome);
            }
            ManifestService::new(gateway, repository)
        }

        fn committed_repository() -> InMemoryDefinitionRepository {
            use destiny_core::ports::WorldDefinitionRepository;
            let mut repository = seeded_repository();
            repository
                .publish_manifest(
                    destiny_core::DefinitionReplacementInput {
                        definitions: destiny_core::ManifestDefinitions {
                            version: DefinitionVersion::new("manifest-prior"),
                            definitions: vec![item_definition(7, "prior", "manifest-prior")],
                        },
                        checked_at: CHECKED_AT,
                        source: SourceMarker::Manifest,
                    },
                    crate::test_support::synthetic_world(DefinitionVersion::new("manifest-prior")),
                )
                .unwrap();
            repository
        }

        #[test]
        fn sync_unchanged_skips_component_and_preserves_metadata() {
            let repository = committed_repository();
            let metadata = repository.current_metadata().unwrap();
            let mut gateway = QueuedManifestGateway::new();
            gateway.queue_index(Ok(destiny_core::ports::ManifestIndex {
                version: DefinitionVersion::new("manifest-prior"),
                component_path: "unused".into(),
                milestone_path: "unused".into(),
                activity_path: "unused".into(),
                modifier_path: "unused".into(),
                objective_path: "unused".into(),
                activity_type_path: "unused".into(),
                stat_path: "unused".into(),
            }));
            let mut service = ManifestService::new(gateway, repository);
            assert_eq!(
                service.sync(Timestamp::new(2_000)).unwrap(),
                crate::manifest::ManifestSyncReport::Unchanged {
                    version: DefinitionVersion::new("manifest-prior"),
                    record_count: 1,
                    world_counts: [1; 5],
                    stat_count: 1,
                }
            );
            assert_eq!(service.gateway().world_calls(), 0);
            assert_eq!(service.gateway().index_calls(), 1);
            assert_eq!(service.gateway().component_calls(), 0);
            assert_eq!(service.repository().publication_attempts(), 1);
            assert_eq!(service.repository().current_metadata().unwrap(), metadata);
            assert_prior_state(service.repository());
        }

        #[test]
        fn sync_changed_and_empty_stores_download_and_report_counts_without_names() {
            for repository in [
                committed_repository(),
                InMemoryDefinitionRepository::default(),
            ] {
                let mut batch = acquisition(
                    VERSION,
                    vec![candidate(42, "item-name-sentinel"), candidate(43, " ")],
                );
                batch.skipped_count = 2;
                let mut service = queued_service([Ok(batch)], repository);
                let report = service.sync(CHECKED_AT).unwrap();
                assert_eq!(
                    report,
                    crate::manifest::ManifestSyncReport::Updated {
                        version: DefinitionVersion::new(VERSION),
                        record_count: 1,
                        skipped: 3,
                        world_counts: [1; 5],
                        world_skipped: [2, 3, 4, 5, 6],
                        stat_count: 1,
                        stat_skipped: 7,
                    }
                );
                assert!(!format!("{report:?}").contains("item-name-sentinel"));
                assert!(!format!("{report:?}").contains("common/destiny2_content"));
                assert_eq!(service.gateway().index_calls(), 1);
                assert_eq!(service.gateway().component_calls(), 1);
                assert_eq!(service.gateway().world_calls(), 1);
                use destiny_core::ports::WorldDefinitionRepository;
                assert!(service.repository().lookup_milestone(1).unwrap().is_some());
                assert!(service.repository().lookup_activity(2).unwrap().is_some());
                assert!(
                    service
                        .repository()
                        .lookup_activity_modifier(3)
                        .unwrap()
                        .is_some()
                );
                assert_eq!(service.repository().definition_count(), 1);
                assert_eq!(
                    service
                        .repository()
                        .current_definition_version()
                        .unwrap()
                        .as_str(),
                    VERSION
                );
            }
        }

        #[test]
        fn sync_index_and_component_failures_leave_catalog_and_metadata_intact() {
            for component_failure in [false, true] {
                let repository = committed_repository();
                let metadata = repository.current_metadata().unwrap();
                let mut gateway = QueuedManifestGateway::new();
                let error = AppError::new(ErrorCategory::ProviderUnavailable);
                if component_failure {
                    gateway.queue_index(Ok(destiny_core::ports::ManifestIndex {
                        version: DefinitionVersion::new(VERSION),
                        component_path: "unused".into(),
                        milestone_path: "unused".into(),
                        activity_path: "unused".into(),
                        modifier_path: "unused".into(),
                        objective_path: "unused".into(),
                        activity_type_path: "unused".into(),
                        stat_path: "unused".into(),
                    }));
                    gateway.queue_component(Err(error));
                } else {
                    gateway.queue_index(Err(error));
                }
                let mut service = ManifestService::new(gateway, repository);
                assert_eq!(service.sync(CHECKED_AT), Err(error));
                assert_eq!(
                    service.gateway().component_calls(),
                    usize::from(component_failure)
                );
                assert_eq!(service.repository().publication_attempts(), 1);
                assert_eq!(service.repository().current_metadata().unwrap(), metadata);
                assert_prior_state(service.repository());
            }
        }

        #[test]
        fn sync_world_failure_or_version_mismatch_preserves_prior_catalogs() {
            use destiny_core::ports::WorldDefinitionRepository;
            for mismatch in [false, true] {
                let mut repository = committed_repository();
                repository
                    .publish_manifest(
                        destiny_core::DefinitionReplacementInput {
                            definitions: destiny_core::ManifestDefinitions {
                                version: DefinitionVersion::new("manifest-prior"),
                                definitions: vec![item_definition(7, "prior", "manifest-prior")],
                            },
                            checked_at: CHECKED_AT,
                            source: SourceMarker::Manifest,
                        },
                        crate::test_support::synthetic_world(DefinitionVersion::new(
                            "manifest-prior",
                        )),
                    )
                    .unwrap();
                let metadata = repository.current_metadata().unwrap();
                let mut gateway = QueuedManifestGateway::new();
                gateway.queue_index(Ok(destiny_core::ports::ManifestIndex {
                    version: DefinitionVersion::new(VERSION),
                    component_path: "unused".into(),
                    milestone_path: "unused".into(),
                    activity_path: "unused".into(),
                    modifier_path: "unused".into(),
                    objective_path: "unused".into(),
                    activity_type_path: "unused".into(),
                    stat_path: "unused".into(),
                }));
                gateway.queue_component(Ok(acquisition(VERSION, vec![candidate(42, "next")])));
                gateway.queue_world(if mismatch {
                    Ok(crate::test_support::synthetic_world(
                        DefinitionVersion::new("wrong-version"),
                    ))
                } else {
                    Err(AppError::new(ErrorCategory::ProviderUnavailable))
                });
                let mut service = ManifestService::new(gateway, repository);
                assert_eq!(
                    service.sync(CHECKED_AT).unwrap_err().category(),
                    if mismatch {
                        ErrorCategory::ProviderValidation
                    } else {
                        ErrorCategory::ProviderUnavailable
                    }
                );
                assert_eq!(service.gateway().world_calls(), 1);
                assert_eq!(service.repository().current_metadata().unwrap(), metadata);
                assert_eq!(
                    service.repository().world_definition_counts().unwrap(),
                    [1; 5]
                );
                assert_prior_state(service.repository());
            }
        }

        #[test]
        fn sync_updated_then_unchanged_downloads_all_components_only_once() {
            let mut gateway = QueuedManifestGateway::new();
            gateway.queue(Ok(acquisition(VERSION, vec![candidate(42, "next")])));
            gateway.queue(Ok(acquisition(
                VERSION,
                vec![candidate(43, "must not download")],
            )));
            let mut service =
                ManifestService::new(gateway, InMemoryDefinitionRepository::default());
            service.sync(CHECKED_AT).unwrap();
            let metadata = service.repository().current_metadata().unwrap();
            assert_eq!(
                service.sync(Timestamp::new(2000)).unwrap(),
                crate::manifest::ManifestSyncReport::Unchanged {
                    version: DefinitionVersion::new(VERSION),
                    record_count: 1,
                    world_counts: [1; 5],
                    stat_count: 1,
                }
            );
            assert_eq!(service.gateway().component_calls(), 1);
            assert_eq!(service.gateway().world_calls(), 1);
            assert_eq!(service.repository().current_metadata().unwrap(), metadata);
        }

        #[test]
        fn sync_metadata_read_failure_is_not_an_empty_store() {
            let repository = committed_repository()
                .with_metadata_failure(AppError::new(ErrorCategory::LocalStorage));
            let mut service = queued_service(
                [Ok(acquisition(VERSION, vec![candidate(42, "Gamma")]))],
                repository,
            );
            assert_eq!(
                service.sync(CHECKED_AT).unwrap_err().category(),
                ErrorCategory::LocalStorage
            );
            assert_eq!(service.gateway().component_calls(), 0);
            assert_eq!(service.repository().publication_attempts(), 1);
            assert_prior_state(service.repository());
        }

        #[test]
        fn sync_rejects_component_version_that_disagrees_with_index() {
            let mut gateway = QueuedManifestGateway::new();
            gateway.queue_index(Ok(destiny_core::ports::ManifestIndex {
                version: DefinitionVersion::new("other-version"),
                component_path: "unused".into(),
                milestone_path: "unused".into(),
                activity_path: "unused".into(),
                modifier_path: "unused".into(),
                objective_path: "unused".into(),
                activity_type_path: "unused".into(),
                stat_path: "unused".into(),
            }));
            gateway.queue_component(Ok(acquisition(VERSION, vec![candidate(42, "Gamma")])));
            let mut service = ManifestService::new(gateway, committed_repository());
            let metadata = service.repository().current_metadata().unwrap();
            assert_eq!(
                service.sync(CHECKED_AT).unwrap_err().category(),
                ErrorCategory::ProviderValidation
            );
            assert_eq!(service.repository().current_metadata().unwrap(), metadata);
            assert_prior_state(service.repository());
        }

        #[test]
        fn successful_acquisition_publishes_the_normalized_set() {
            let mut service = queued_service(
                [Ok(acquisition(
                    VERSION,
                    vec![candidate(3, "  Ri fle  "), candidate(1, "Alpha")],
                ))],
                InMemoryDefinitionRepository::default(),
            );

            let outcome = service.acquire_and_publish(CHECKED_AT).unwrap();

            assert_eq!(outcome.version.as_str(), VERSION);
            assert_eq!(outcome.definition_count, 2);
            assert_eq!(outcome.revision.value(), 1);

            let repository = service.repository();
            assert_eq!(
                repository.current_definition_version().unwrap().as_str(),
                VERSION
            );
            assert_eq!(repository.definition_count(), 2);
            assert_eq!(repository.publication_attempts(), 1);
            let metadata = repository.current_metadata().unwrap().unwrap();
            assert_eq!(metadata.dataset, Dataset::ManifestDefinitions);
            assert_eq!(metadata.source, SourceMarker::Manifest);
            assert_eq!(metadata.checked_at, CHECKED_AT);
            assert_eq!(metadata.revision, outcome.revision);
            assert_eq!(metadata.definition_version.as_str(), VERSION);
            assert_eq!(
                repository.lookup_definition(ItemHash::new(3)).unwrap(),
                Resolution::Unique(item_definition(3, "Ri fle", VERSION))
            );
            assert_eq!(
                repository.lookup_definition(ItemHash::new(1)).unwrap(),
                Resolution::Unique(item_definition(1, "Alpha", VERSION))
            );
            assert_eq!(
                repository.lookup_definition(ItemHash::new(99)).unwrap(),
                Resolution::Missing
            );
        }

        #[test]
        fn repeated_publication_of_the_same_acquisition_is_idempotent() {
            let batch = acquisition(VERSION, vec![candidate(2, "Beta"), candidate(1, "Alpha")]);
            let mut service = queued_service(
                [Ok(batch.clone()), Ok(batch)],
                InMemoryDefinitionRepository::default(),
            );

            let first = service.acquire_and_publish(CHECKED_AT).unwrap();
            let version = service.repository().current_definition_version().unwrap();
            let later = Timestamp::new(CHECKED_AT.seconds() + 1);
            let second = service.acquire_and_publish(later).unwrap();

            // An identical refresh at a later timestamp is idempotent: the
            // revision is retained, so the minimal outcome is unchanged.
            assert_eq!(first, second);
            assert_eq!(first.revision.value(), 1);
            assert_eq!(
                service.repository().current_definition_version().unwrap(),
                version
            );
            assert_eq!(service.repository().definition_count(), 2);
            assert_eq!(service.repository().publication_attempts(), 2);
            let metadata = service.repository().current_metadata().unwrap().unwrap();
            assert_eq!(metadata.revision.value(), 1);
            assert_eq!(metadata.checked_at, later);
        }

        #[test]
        fn changed_checked_at_retains_revision_and_refreshes_metadata() {
            let batch = acquisition(VERSION, vec![candidate(1, "Alpha")]);
            let mut service = queued_service(
                [Ok(batch.clone()), Ok(batch)],
                InMemoryDefinitionRepository::default(),
            );

            let first = service.acquire_and_publish(CHECKED_AT).unwrap();
            let later = Timestamp::new(CHECKED_AT.seconds() + 1);
            let second = service.acquire_and_publish(later).unwrap();

            assert_eq!(first.revision.value(), 1);
            assert_eq!(second.revision.value(), 1);
            let metadata = service.repository().current_metadata().unwrap().unwrap();
            assert_eq!(metadata.checked_at, later);
            assert_eq!(metadata.revision, second.revision);
            assert_eq!(metadata.source, SourceMarker::Manifest);
        }

        #[test]
        fn gateway_failure_preserves_prior_repository_state() {
            let mut service = queued_service(
                [Err(AppError::new(ErrorCategory::ProviderUnavailable))],
                seeded_repository(),
            );

            assert_eq!(
                service
                    .acquire_and_publish(CHECKED_AT)
                    .unwrap_err()
                    .category(),
                ErrorCategory::ProviderUnavailable
            );
            assert_eq!(service.repository().publication_attempts(), 0);
            assert_prior_state(service.repository());
        }

        #[test]
        fn repository_rejection_preserves_prior_repository_state() {
            let repository = seeded_repository()
                .with_publication_failure(AppError::new(ErrorCategory::ProviderUnavailable));
            let mut service = queued_service(
                [Ok(acquisition(VERSION, vec![candidate(42, "Gamma")]))],
                repository,
            );

            assert_eq!(
                service
                    .acquire_and_publish(CHECKED_AT)
                    .unwrap_err()
                    .category(),
                ErrorCategory::ProviderUnavailable
            );
            assert_eq!(service.repository().publication_attempts(), 1);
            assert_prior_state(service.repository());
        }
    }
}
