//! Public weekly reset reads resolved against the committed Manifest.
//!
//! Each milestone retains its own optional UTC reset window. Modifier
//! descriptions, including `{var:<hash>}` templates, are returned verbatim;
//! this service never expands templates or caches public snapshots.

use destiny_core::{
    AppError, DefinitionVersion, ErrorCategory, PublicMilestonesGateway, Timestamp,
    calendar::{ActivityReference, Calendar, WeekWindow, week_index},
    ports::WorldDefinitionRepository,
};

/// Exact hash lookup result. Unlike semantic `Resolution`, these lookups cannot
/// be ambiguous: a catalog contains at most one definition per hash.
#[derive(Clone, Eq, PartialEq)]
pub enum DefinitionResolution {
    Resolved { name: String, description: String },
    Unresolved,
}

// Provider text is presentation data, not diagnostic data.
impl std::fmt::Debug for DefinitionResolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resolved { .. } => f.debug_struct("Resolved").finish_non_exhaustive(),
            Self::Unresolved => f.write_str("Unresolved"),
        }
    }
}

fn resolved(name: &str, description: &str) -> DefinitionResolution {
    DefinitionResolution::Resolved {
        name: name.into(),
        description: description.into(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeeklyModifier {
    pub hash: u32,
    pub definition: DefinitionResolution,
    /// Derived, name-based English hint, not an API taxonomy.
    pub category: crate::weekly_personal::ModifierCategory,
}

impl WeeklyModifier {
    pub fn new(hash: u32, definition: DefinitionResolution) -> Self {
        let category = match &definition {
            DefinitionResolution::Resolved { name, .. } => {
                crate::weekly_personal::modifier_category(name)
            }
            DefinitionResolution::Unresolved => crate::weekly_personal::ModifierCategory::Other,
        };
        Self {
            hash,
            definition,
            category,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeeklyActivity {
    pub hash: u32,
    pub definition: DefinitionResolution,
    pub modifiers: Vec<WeeklyModifier>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeeklyMilestone {
    pub hash: u32,
    pub order: u32,
    pub definition: DefinitionResolution,
    pub start: Option<Timestamp>,
    pub end: Option<Timestamp>,
    pub activities: Vec<WeeklyActivity>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RotationOutcome {
    Predicted(Vec<ActivityReference>),
    NoData {
        coverage_start: Timestamp,
        coverage_note: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeeklyRotation {
    pub key: String,
    pub title: String,
    pub outcome: RotationOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeeklyCalendar {
    pub window: WeekWindow,
    pub rotations: Vec<WeeklyRotation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeeklyReset {
    pub current_week: bool,
    pub calendar: WeeklyCalendar,
    pub definition_version: DefinitionVersion,
    pub milestones: Vec<WeeklyMilestone>,
}

/// Resolves a fresh public snapshot without account state or repository writes.
pub struct WeeklyResetService<G, R> {
    gateway: G,
    repository: R,
}

impl<G: PublicMilestonesGateway, R: WorldDefinitionRepository> WeeklyResetService<G, R> {
    pub fn new(gateway: G, repository: R) -> Self {
        Self {
            gateway,
            repository,
        }
    }

    pub fn gateway(&self) -> &G {
        &self.gateway
    }

    pub fn repository(&self) -> &R {
        &self.repository
    }

    /// Requires committed Manifest metadata and all three populated world tables
    /// before calling the provider. Typed gateway and storage failures propagate
    /// unchanged; absent individual hashes are explicit unresolved outcomes.
    pub fn weekly_reset(
        &mut self,
        calendar: &Calendar,
        requested: Timestamp,
        now: Timestamp,
    ) -> Result<WeeklyReset, AppError> {
        let metadata = self
            .repository
            .current_metadata()?
            .ok_or_else(|| AppError::new(ErrorCategory::DefinitionsUnavailable))?;
        if self.repository.world_definition_counts()?[..3].contains(&0) {
            return Err(AppError::new(ErrorCategory::DefinitionsUnavailable));
        }
        let week = calendar
            .week_at(requested)
            .map_err(|_| AppError::new(ErrorCategory::ClockOverflow))?;
        let current_week = week.index == week_index(calendar.reference_reset(), now);
        let calendar = WeeklyCalendar {
            window: week.window,
            rotations: week
                .rotations
                .into_iter()
                .map(|prediction| WeeklyRotation {
                    key: prediction.rotation.key().into(),
                    title: prediction.rotation.title().into(),
                    outcome: match prediction.outcome {
                        destiny_core::calendar::RotationOutcome::Predicted(activities) => {
                            RotationOutcome::Predicted(activities.to_vec())
                        }
                        destiny_core::calendar::RotationOutcome::NoData {
                            coverage_start,
                            coverage_note,
                        } => RotationOutcome::NoData {
                            coverage_start,
                            coverage_note: coverage_note.into(),
                        },
                    },
                })
                .collect(),
        };
        if !current_week {
            return Ok(WeeklyReset {
                definition_version: metadata.definition_version,
                current_week,
                calendar,
                milestones: Vec::new(),
            });
        }
        let mut snapshot = self.gateway.load_public_milestones()?;
        snapshot.validate()?;
        snapshot
            .milestones
            .sort_by_key(|m| (m.order, m.milestone_hash));
        let mut milestones = Vec::with_capacity(snapshot.milestones.len());
        for milestone in snapshot.milestones {
            let definition = self
                .repository
                .lookup_milestone(milestone.milestone_hash)?
                .map_or(DefinitionResolution::Unresolved, |d| {
                    resolved(d.name(), d.description())
                });
            let mut activities = Vec::with_capacity(milestone.activities.len());
            for activity in milestone.activities {
                let definition = self
                    .repository
                    .lookup_activity(activity.activity_hash)?
                    .map_or(DefinitionResolution::Unresolved, |d| {
                        resolved(d.name(), d.description())
                    });
                let mut modifiers = Vec::with_capacity(activity.modifier_hashes.len());
                for hash in activity.modifier_hashes {
                    let definition = self
                        .repository
                        .lookup_activity_modifier(hash)?
                        .map_or(DefinitionResolution::Unresolved, |d| {
                            resolved(d.name(), d.description())
                        });
                    modifiers.push(WeeklyModifier::new(hash, definition));
                }
                activities.push(WeeklyActivity {
                    hash: activity.activity_hash,
                    definition,
                    modifiers,
                });
            }
            milestones.push(WeeklyMilestone {
                hash: milestone.milestone_hash,
                order: milestone.order,
                definition,
                start: milestone.start,
                end: milestone.end,
                activities,
            });
        }
        Ok(WeeklyReset {
            current_week,
            calendar,
            definition_version: metadata.definition_version,
            milestones,
        })
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use crate::test_support::InMemoryDefinitionRepository;
    use destiny_core::{
        ActivityDefinition, ActivityModifierDefinition, DefinitionReplacementInput,
        DefinitionRepository, ManifestDefinitions, MilestoneDefinition, PublicMilestone,
        PublicMilestoneActivity, PublicMilestonesSnapshot, SourceMarker, WorldDefinitionTable,
        WorldDefinitionsAcquisition,
    };

    struct Gateway {
        result: Result<PublicMilestonesSnapshot, AppError>,
        calls: usize,
    }

    impl PublicMilestonesGateway for Gateway {
        fn load_public_milestones(&mut self) -> Result<PublicMilestonesSnapshot, AppError> {
            self.calls += 1;
            self.result.clone()
        }
    }

    struct RepositoryDouble {
        inner: InMemoryDefinitionRepository,
        counts: [usize; 5],
        failure: Option<ReadFailure>,
    }

    #[derive(Clone, Copy)]
    enum ReadFailure {
        Metadata,
        Counts,
        Lookup,
    }

    impl DefinitionRepository for RepositoryDouble {
        fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
            self.inner.current_definition_version()
        }

        fn lookup_definition(
            &self,
            hash: destiny_core::ItemHash,
        ) -> Result<destiny_core::Resolution<destiny_core::ItemDefinition>, AppError> {
            self.inner.lookup_definition(hash)
        }

        fn current_metadata(&self) -> Result<Option<destiny_core::SyncMetadata>, AppError> {
            if matches!(self.failure, Some(ReadFailure::Metadata)) {
                return Err(AppError::new(ErrorCategory::LocalStorage));
            }
            self.inner.current_metadata()
        }

        fn replace_definitions(
            &mut self,
            input: DefinitionReplacementInput,
        ) -> Result<destiny_core::SyncMetadata, AppError> {
            self.inner.replace_definitions(input)
        }
    }

    impl WorldDefinitionRepository for RepositoryDouble {
        fn lookup_item_description(
            &self,
            hash: destiny_core::ItemHash,
        ) -> Result<Option<String>, AppError> {
            self.inner.lookup_item_description(hash)
        }
        fn lookup_loadout_name(&self, hash: u32) -> Result<Option<String>, AppError> {
            self.inner.lookup_loadout_name(hash)
        }
        fn lookup_objective(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_core::ObjectiveDefinition>, AppError> {
            self.inner.lookup_objective(hash)
        }
        fn lookup_activity_type(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_core::ActivityTypeDefinition>, AppError> {
            self.inner.lookup_activity_type(hash)
        }
        fn world_definition_counts(&self) -> Result<[usize; 5], AppError> {
            if matches!(self.failure, Some(ReadFailure::Counts)) {
                return Err(AppError::new(ErrorCategory::LocalStorage));
            }
            Ok(self.counts)
        }

        fn lookup_milestone(&self, hash: u32) -> Result<Option<MilestoneDefinition>, AppError> {
            if matches!(self.failure, Some(ReadFailure::Lookup)) {
                return Err(AppError::new(ErrorCategory::LocalStorage));
            }
            if self.counts[0] == 0 {
                return Ok(None);
            }
            self.inner.lookup_milestone(hash)
        }

        fn lookup_activity(&self, hash: u32) -> Result<Option<ActivityDefinition>, AppError> {
            if self.counts[1] == 0 {
                return Ok(None);
            }
            self.inner.lookup_activity(hash)
        }

        fn lookup_activity_modifier(
            &self,
            hash: u32,
        ) -> Result<Option<ActivityModifierDefinition>, AppError> {
            if self.counts[2] == 0 {
                return Ok(None);
            }
            self.inner.lookup_activity_modifier(hash)
        }

        fn publish_manifest(
            &mut self,
            items: DefinitionReplacementInput,
            world: WorldDefinitionsAcquisition,
        ) -> Result<destiny_core::SyncMetadata, AppError> {
            self.inner.publish_manifest(items, world)
        }
    }

    fn calendar() -> destiny_core::calendar::Calendar {
        use destiny_core::calendar::{ActivityReference, Calendar, Rotation};
        Calendar::new(
            Timestamp::new(10),
            vec![
                Rotation::new(
                    "raid",
                    "Raids",
                    0,
                    Timestamp::new(10),
                    "Verified from reference",
                    vec![vec![ActivityReference::new("alpha", "Alpha", 42).unwrap()]],
                )
                .unwrap(),
            ],
        )
        .unwrap()
    }

    #[test]
    fn requested_weeks_gate_live_calls_and_always_predict() {
        use super::RotationOutcome;
        let calendar = calendar();
        let now = Timestamp::new(700_000);
        let mut service = service(vec![milestone(1, 0)]);
        let current = service.weekly_reset(&calendar, now, now).unwrap();
        assert!(current.current_week);
        assert_eq!(service.gateway.calls, 1);
        assert_eq!(current.milestones.len(), 1);
        assert!(matches!(
            current.calendar.rotations[0].outcome,
            RotationOutcome::Predicted(_)
        ));
        let other = service
            .weekly_reset(&calendar, Timestamp::new(10), now)
            .unwrap();
        assert!(!other.current_week);
        assert!(other.milestones.is_empty());
        assert_eq!(service.gateway.calls, 1);
        assert!(matches!(
            other.calendar.rotations[0].outcome,
            RotationOutcome::Predicted(_)
        ));
        let precoverage = destiny_core::calendar::Calendar::new(
            Timestamp::new(10),
            vec![
                destiny_core::calendar::Rotation::new(
                    "raid",
                    "Raids",
                    0,
                    Timestamp::new(604_810),
                    "Starts later",
                    calendar.rotations()[0].entries().to_vec(),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let result = service
            .weekly_reset(&precoverage, Timestamp::new(10), now)
            .unwrap();
        assert!(
            matches!(&result.calendar.rotations[0].outcome, RotationOutcome::NoData { coverage_note, .. } if coverage_note == "Starts later")
        );
        assert_eq!(service.gateway.calls, 1);
    }

    #[test]
    fn each_empty_world_table_prevents_acquisition() {
        for counts in [[0, 1, 1, 1, 1], [1, 0, 1, 1, 1], [1, 1, 0, 1, 1]] {
            let original = service(vec![milestone(1, 0)]);
            let mut service = WeeklyResetService::new(
                original.gateway,
                RepositoryDouble {
                    inner: original.repository,
                    counts,
                    failure: None,
                },
            );
            assert!(service.repository.current_metadata().unwrap().is_some());
            assert_eq!(
                service.repository.world_definition_counts().unwrap(),
                counts
            );
            assert_eq!(
                [
                    service.repository.lookup_milestone(1).unwrap().is_some(),
                    service.repository.lookup_activity(2).unwrap().is_some(),
                    service
                        .repository
                        .lookup_activity_modifier(3)
                        .unwrap()
                        .is_some(),
                ],
                [counts[0] > 0, counts[1] > 0, counts[2] > 0]
            );
            assert_eq!(
                service
                    .weekly_reset(&calendar(), Timestamp::new(10), Timestamp::new(10))
                    .unwrap_err(),
                AppError::new(ErrorCategory::DefinitionsUnavailable)
            );
            assert_eq!(service.gateway.calls, 0, "counts: {counts:?}");
        }
    }

    #[test]
    fn storage_read_errors_propagate_without_becoming_unresolved() {
        for (failure, expected_calls) in [
            (ReadFailure::Metadata, 0),
            (ReadFailure::Counts, 0),
            (ReadFailure::Lookup, 1),
        ] {
            let original = service(vec![milestone(1, 0)]);
            let mut service = WeeklyResetService::new(
                original.gateway,
                RepositoryDouble {
                    inner: original.repository,
                    counts: [1; 5],
                    failure: Some(failure),
                },
            );
            assert_eq!(
                service
                    .weekly_reset(&calendar(), Timestamp::new(10), Timestamp::new(10))
                    .unwrap_err(),
                AppError::new(ErrorCategory::LocalStorage)
            );
            assert_eq!(service.gateway.calls, expected_calls);
        }
    }

    fn items(version: DefinitionVersion) -> ManifestDefinitions {
        ManifestDefinitions {
            version: version.clone(),
            definitions: vec![destiny_core::ItemDefinition {
                does_postmaster_pull_have_side_effects: None,
                is_instance_item: None,
                max_stack_size: None,
                stack_unique_label: None,
                hash: destiny_core::ItemHash::new(1),
                bucket_hash: None,
                name: "Item".into(),
                item_type: None,
                tier: None,
                element: None,
                slot: None,
                class_type: None,
                plug_category: None,
                tags: vec![],
                version,
            }],
        }
    }

    fn repository() -> InMemoryDefinitionRepository {
        let mut repository = InMemoryDefinitionRepository::default();
        let version = DefinitionVersion::new("world-v1");
        repository
            .publish_manifest(
                DefinitionReplacementInput {
                    definitions: items(version.clone()),
                    checked_at: Timestamp::new(0),
                    source: SourceMarker::Manifest,
                },
                WorldDefinitionsAcquisition {
                    item_descriptions: Vec::new(),
                    loadout_names: WorldDefinitionTable {
                        definitions: vec![],
                        skipped_count: 0,
                    },
                    stats: crate::test_support::synthetic_world(version.clone()).stats,
                    objectives: crate::test_support::synthetic_world(version.clone()).objectives,
                    activity_types: crate::test_support::synthetic_world(version.clone())
                        .activity_types,
                    version,
                    milestones: WorldDefinitionTable {
                        definitions: vec![
                            MilestoneDefinition::new(
                                1,
                                "milestone-name-sentinel".into(),
                                "milestone-description-sentinel".into(),
                                0,
                            )
                            .unwrap(),
                        ],
                        skipped_count: 0,
                    },
                    activities: WorldDefinitionTable {
                        definitions: vec![
                            ActivityDefinition::new(
                                2,
                                "activity-name-sentinel".into(),
                                "activity-description-sentinel".into(),
                                0,
                                0,
                            )
                            .unwrap(),
                        ],
                        skipped_count: 0,
                    },
                    modifiers: WorldDefinitionTable {
                        definitions: vec![
                            ActivityModifierDefinition::new(
                                3,
                                "modifier-name-sentinel".into(),
                                "modifier-description-sentinel {var:42}".into(),
                            )
                            .unwrap(),
                        ],
                        skipped_count: 0,
                    },
                },
            )
            .unwrap();
        repository
    }

    fn milestone(hash: u32, order: u32) -> PublicMilestone {
        PublicMilestone {
            milestone_hash: hash,
            order,
            start: Some(Timestamp::new(10)),
            end: Some(Timestamp::new(20)),
            activities: vec![PublicMilestoneActivity {
                activity_hash: 2,
                modifier_hashes: vec![3],
                challenge_objective_hashes: vec![],
            }],
        }
    }

    fn service(
        milestones: Vec<PublicMilestone>,
    ) -> WeeklyResetService<Gateway, InMemoryDefinitionRepository> {
        WeeklyResetService::new(
            Gateway {
                result: Ok(PublicMilestonesSnapshot { milestones }),
                calls: 0,
            },
            repository(),
        )
    }

    #[test]
    fn definitions_unavailable_never_calls_gateway() {
        let mut service = service(vec![]);
        service.repository = InMemoryDefinitionRepository::default();
        assert_eq!(
            service
                .weekly_reset(&calendar(), Timestamp::new(10), Timestamp::new(10))
                .unwrap_err()
                .category(),
            ErrorCategory::DefinitionsUnavailable
        );
        assert_eq!(service.gateway.calls, 0);
        // Metadata without world tables also fails before acquisition.
        service.repository = repository();
        service
            .repository
            .replace_definitions(DefinitionReplacementInput {
                definitions: items(DefinitionVersion::new("items-only")),
                checked_at: Timestamp::new(0),
                source: SourceMarker::Manifest,
            })
            .unwrap();
        assert_eq!(
            service
                .weekly_reset(&calendar(), Timestamp::new(10), Timestamp::new(10))
                .unwrap_err()
                .category(),
            ErrorCategory::DefinitionsUnavailable
        );
        assert_eq!(service.gateway.calls, 0);
    }

    #[test]
    fn resolves_all_and_preserves_templates_and_windows() {
        let mut service = service(vec![milestone(1, 7)]);
        let result = service
            .weekly_reset(&calendar(), Timestamp::new(10), Timestamp::new(10))
            .unwrap();
        assert_eq!(
            result.definition_version,
            DefinitionVersion::new("world-v1")
        );
        let milestone = &result.milestones[0];
        assert_eq!(
            milestone.definition,
            resolved("milestone-name-sentinel", "milestone-description-sentinel")
        );
        assert_eq!(
            (milestone.start, milestone.end),
            (Some(Timestamp::new(10)), Some(Timestamp::new(20)))
        );
        assert_eq!(
            milestone.activities[0].definition,
            resolved("activity-name-sentinel", "activity-description-sentinel")
        );
        assert_eq!(
            milestone.activities[0].modifiers[0].definition,
            resolved(
                "modifier-name-sentinel",
                "modifier-description-sentinel {var:42}"
            )
        );
        let diagnostic = format!("{result:?}");
        assert!(diagnostic.contains("hash: 1"));
        for text in [
            "milestone-name-sentinel",
            "milestone-description-sentinel",
            "activity-name-sentinel",
            "activity-description-sentinel",
            "modifier-name-sentinel",
            "modifier-description-sentinel {var:42}",
        ] {
            assert!(!diagnostic.contains(text), "Debug leaked {text}");
        }
    }

    #[test]
    fn unresolved_hashes_are_explicit_and_provider_child_order_is_preserved() {
        let mut missing = milestone(99, 0);
        missing.activities.insert(
            0,
            PublicMilestoneActivity {
                activity_hash: 88,
                modifier_hashes: vec![77, 3],
                challenge_objective_hashes: vec![],
            },
        );
        let result = service(vec![missing])
            .weekly_reset(&calendar(), Timestamp::new(10), Timestamp::new(10))
            .unwrap();
        let milestone = &result.milestones[0];
        assert_eq!(milestone.hash, 99);
        assert_eq!(milestone.definition, DefinitionResolution::Unresolved);
        assert_eq!(
            milestone
                .activities
                .iter()
                .map(|a| a.hash)
                .collect::<Vec<_>>(),
            vec![88, 2]
        );
        assert_eq!(
            milestone.activities[0].definition,
            DefinitionResolution::Unresolved
        );
        let modifiers = &milestone.activities[0].modifiers;
        assert_eq!(
            modifiers.iter().map(|m| m.hash).collect::<Vec<_>>(),
            vec![77, 3]
        );
        assert_eq!(modifiers[0].definition, DefinitionResolution::Unresolved);
        assert!(matches!(
            modifiers[1].definition,
            DefinitionResolution::Resolved { .. }
        ));
    }

    #[test]
    fn milestone_order_is_order_then_hash() {
        let result = service(vec![milestone(9, 2), milestone(4, 1), milestone(1, 1)])
            .weekly_reset(&calendar(), Timestamp::new(10), Timestamp::new(10))
            .unwrap();
        assert_eq!(
            result
                .milestones
                .iter()
                .map(|m| (m.order, m.hash))
                .collect::<Vec<_>>(),
            vec![(1, 1), (1, 4), (2, 9)]
        );
    }

    #[test]
    fn gateway_error_propagates() {
        let mut service = service(vec![]);
        let error = AppError::new(ErrorCategory::ProviderUnavailable);
        service.gateway.result = Err(error.clone());
        assert_eq!(
            service
                .weekly_reset(&calendar(), Timestamp::new(10), Timestamp::new(10))
                .unwrap_err(),
            error
        );
    }

    #[test]
    fn empty_snapshot_and_repeated_reads_are_not_cached() {
        let mut service = service(vec![]);
        assert!(
            service
                .weekly_reset(&calendar(), Timestamp::new(10), Timestamp::new(10))
                .unwrap()
                .milestones
                .is_empty()
        );
        service.gateway.result = Ok(PublicMilestonesSnapshot {
            milestones: vec![milestone(1, 0)],
        });
        assert_eq!(
            service
                .weekly_reset(&calendar(), Timestamp::new(10), Timestamp::new(10))
                .unwrap()
                .milestones
                .len(),
            1
        );
        assert_eq!(service.gateway.calls, 2);
    }

    #[test]
    fn invalid_snapshot_is_rejected() {
        let error = service(vec![milestone(1, 0), milestone(1, 1)])
            .weekly_reset(&calendar(), Timestamp::new(10), Timestamp::new(10))
            .unwrap_err();
        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
    }
}
