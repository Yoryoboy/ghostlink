//! Offline profile synchronization orchestration.
//!
//! The service is provider- and transport-neutral. It reads the committed
//! projection through the core-owned `InventoryRepository`, asks the core-owned
//! `ProfileGateway` for one complete normalized candidate when a refresh is
//! warranted, verifies that the candidate belongs to the membership it resolved,
//! and commits it atomically. It never retains or surfaces raw payloads, upstream
//! messages, credential material, or membership identifiers; failures stay
//! sanitized [`AppError`] categories.
//!
//! The caller supplies all policy input: the sync `reason`, the freshness
//! `policy`, and the explicit `now` timestamp. The service reads no ambient
//! clock. It never stamps `checked_at`, `source`, or `definition_version`, and
//! never computes a revision: the gateway stamps acquisition metadata and the
//! repository owns provenance, structural validation, and revision allocation.

use destiny_core::{
    AppError, Dataset, ErrorCategory, FreshnessInput, FreshnessPolicy, FreshnessStatus,
    InventoryRepository, ProfileGateway, ProfileSyncOutcome, ProfileSyncReason, Timestamp,
};

/// Orchestrates one offline profile synchronization decision.
///
/// The decision is fail-closed and never mutates the repository before a
/// candidate is validated:
///
/// - The freshness gate is advisory only. An [`ProfileSyncReason::Ordinary`]
///   request whose committed projection is already [`FreshnessStatus::Fresh`]
///   skips acquisition entirely, so a fresh projection is never re-fetched just
///   to confirm it. Every other combination refreshes: `Ordinary` over a
///   missing, stale, or mis-sourced projection, and `Forced`/`Preflight`/
///   `PostAction` regardless of freshness, because an explicit user action, a
///   mutation preflight, or a just-completed action must never read a projection
///   it cannot trust to be current.
/// - A refresh resolves the effective membership, loads exactly one complete
///   candidate for it, and verifies the candidate carries that same membership
///   before publication. A candidate for a different membership is rejected as
///   [`ErrorCategory::ProviderValidation`] and never replaces the prior
///   projection, so a partial or mismatched provider response can never publish
///   another account's inventory.
///
/// Gateway and repository errors propagate unchanged, preserving their exact
/// category and sanitization.
pub struct ProfileSyncService<G, R> {
    gateway: G,
    repository: R,
    lookup_plug: fn(
        &R,
        destiny_core::ItemHash,
    )
        -> Result<destiny_core::Resolution<destiny_core::ItemDefinition>, AppError>,
}

impl<G, R> ProfileSyncService<G, R>
where
    G: ProfileGateway,
    R: InventoryRepository,
{
    pub fn new(gateway: G, repository: R) -> Self {
        Self {
            gateway,
            repository,
            lookup_plug: |_, _| Ok(destiny_core::Resolution::Missing),
        }
    }

    /// Compose profile publication with read access to the same definition store.
    /// The legacy inventory-only constructor treats all definitions as unknown.
    pub fn new_with_definitions(gateway: G, repository: R) -> Self
    where
        R: destiny_core::DefinitionRepository,
    {
        Self {
            gateway,
            repository,
            lookup_plug: |repository, hash| repository.lookup_definition(hash),
        }
    }

    /// Transfers the authenticated gateway and committed repository together.
    pub fn into_parts(self) -> (G, R) {
        (self.gateway, self.repository)
    }

    /// Borrows the repository so callers can read the visible projection.
    pub fn repository(&self) -> &R {
        &self.repository
    }

    /// Mutably borrows the repository for test or composition setup.
    pub fn repository_mut(&mut self) -> &mut R {
        &mut self.repository
    }

    /// Decides and, when warranted, performs one profile synchronization.
    ///
    /// The caller supplies the sync `reason`, the freshness `policy`, and the
    /// explicit `now` timestamp; the service never reads an ambient clock. The
    /// committed projection is evaluated first, and an `Ordinary` request over a
    /// fresh projection returns [`ProfileSyncOutcome::SkippedFresh`] without
    /// calling the gateway or touching the repository. Otherwise the gateway
    /// resolves the membership and loads one complete candidate, the candidate
    /// is verified against that membership, and the verified candidate is
    /// committed through the repository, which owns revision retention and
    /// advancement. The outcome carries repository metadata only.
    ///
    /// This method never sets or overrides `checked_at`, `source`, or
    /// `definition_version`, and never computes a revision. It returns the
    /// [`ProfileSyncOutcome`] the repository reports, and clones no projection
    /// on the success path.
    ///
    /// The sync `reason` is forwarded to the gateway's reason-scoped read
    /// methods, so a gateway may bound retries differently for an ordinary read
    /// than for a preflight or post-action mutation observation. A gateway that
    /// does not override the scoped methods keeps the conservative one-shot
    /// behavior through the trait defaults.
    pub fn sync(
        &mut self,
        reason: ProfileSyncReason,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<ProfileSyncOutcome, AppError> {
        let committed = self.repository.load_current()?;
        let status = FreshnessInput {
            dataset: Dataset::ProfileInventory,
            checked_at: committed
                .as_ref()
                .map(|snapshot| snapshot.metadata.checked_at),
            source: committed.as_ref().map(|snapshot| snapshot.metadata.source),
            now,
            policy,
        }
        .status();

        if reason == ProfileSyncReason::Ordinary && status == FreshnessStatus::Fresh {
            // `Fresh` requires a committed `checked_at`, so a fresh status
            // implies the snapshot is present; moving its metadata avoids
            // cloning the projection. No gateway call and no mutation happen.
            if let Some(snapshot) = committed {
                return Ok(ProfileSyncOutcome::SkippedFresh {
                    metadata: snapshot.metadata,
                });
            }
        }

        let membership = self.gateway.resolve_membership_scoped(reason)?;
        let mut candidate = self.gateway.load_inventory_scoped(&membership, reason)?;
        // The candidate must belong to the membership it was resolved for, so a
        // mismatched or partial provider response can never be published.
        if candidate.membership_id != *membership.membership_id()
            || candidate.membership_type != membership.membership_type()
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        // Resolve each distinct plug once before publication. Lookup failures
        // abort without writing; missing or ambiguous definitions are unknown
        // and retained. A known definition without a category is not a perk.
        let mut categories = std::collections::HashMap::new();
        for hash in candidate.items.iter().flat_map(|item| {
            item.plugs
                .iter()
                .map(|plug| plug.plug_hash)
                .chain(item.available_plugs.iter().map(|plug| plug.plug_hash))
        }) {
            if let std::collections::hash_map::Entry::Vacant(entry) = categories.entry(hash) {
                let category = match (self.lookup_plug)(&self.repository, hash)? {
                    destiny_core::Resolution::Unique(definition) => {
                        Some(definition.plug_category.unwrap_or_default())
                    }
                    _ => None,
                };
                entry.insert(category);
            }
        }
        for item in &mut candidate.items {
            item.retain_searchable_plugs(|hash| {
                categories
                    .get(&hash)
                    .and_then(|category| category.as_deref())
            });
        }
        let snapshot = self.repository.replace_current(candidate)?;
        Ok(ProfileSyncOutcome::Refreshed {
            metadata: snapshot.metadata,
        })
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    mod behavioral {
        use crate::sync::ProfileSyncService;
        use crate::test_support::{InMemoryInventoryRepository, QueuedProfileGateway};
        use destiny_core::{
            AppError, DefinitionVersion, DestinyMembership, ErrorCategory, FreshnessPolicy,
            InventoryRepository, InventorySnapshot, InventorySnapshotInput, ItemHash, ItemInstance,
            ItemLocation, MembershipId, MembershipType, ProfileGateway, ProfileSyncReason,
            SourceMarker, SyncRevision, Timestamp,
        };
        use std::cell::RefCell;
        use std::rc::Rc;

        const NOW: Timestamp = Timestamp::new(10_000);
        const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };
        const DEFINITION_VERSION: &str = "definitions-v1";

        /// Counters and `requested_memberships` live in the shared
        /// [`QueuedProfileGateway`], which the service owns by value. This
        /// read-only spy delegates to that same fake so an observation handle
        /// can assert call counts after the service has taken ownership. It
        /// queues nothing and invents no behavior.
        struct SharedGateway(Rc<RefCell<QueuedProfileGateway>>);

        impl ProfileGateway for SharedGateway {
            fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
                self.0.borrow_mut().resolve_membership()
            }

            fn load_inventory(
                &mut self,
                membership: &DestinyMembership,
            ) -> Result<InventorySnapshotInput, AppError> {
                self.0.borrow_mut().load_inventory(membership)
            }
        }

        type Service = ProfileSyncService<SharedGateway, InMemoryInventoryRepository>;

        fn gateway() -> Rc<RefCell<QueuedProfileGateway>> {
            Rc::new(RefCell::new(QueuedProfileGateway::new()))
        }

        fn harness(
            gateway: &Rc<RefCell<QueuedProfileGateway>>,
            seed: Option<InventorySnapshotInput>,
        ) -> Service {
            let mut repository = InMemoryInventoryRepository::new();
            if let Some(input) = seed {
                repository
                    .replace_current(input)
                    .expect("valid seed snapshot");
            }
            ProfileSyncService::new(SharedGateway(Rc::clone(gateway)), repository)
        }

        fn membership_with(identifier: &str, membership_type: u32) -> DestinyMembership {
            DestinyMembership::new(
                MembershipType::new(membership_type),
                MembershipId::new(identifier),
            )
            .expect("valid membership")
        }

        fn membership() -> DestinyMembership {
            membership_with("4611686018484639825", 3)
        }

        fn inventory(
            membership: &DestinyMembership,
            checked_at: Timestamp,
            quantity: u32,
        ) -> InventorySnapshotInput {
            InventorySnapshotInput {
                membership_id: membership.membership_id().clone(),
                membership_type: membership.membership_type(),
                definition_version: DefinitionVersion::new(DEFINITION_VERSION),
                checked_at,
                source: SourceMarker::Profile,
                characters: Vec::new(),
                items: vec![ItemInstance {
                    available_plugs: Vec::new(),
                    instance_id: "instance-1".into(),
                    item_hash: ItemHash::new(7),
                    location: ItemLocation::Vault,
                    quantity,
                    equipped: false,
                    locked: false,
                    power: None,
                    stats: Vec::new(),
                    plugs: Vec::new(),
                    perks: Vec::new(),
                }],
            }
        }

        fn committed_snapshot(service: &Service) -> InventorySnapshot {
            service
                .repository()
                .load_current()
                .expect("repository read")
                .expect("committed snapshot")
        }

        fn fresh_seed(membership: &DestinyMembership) -> InventorySnapshotInput {
            inventory(membership, Timestamp::new(NOW.seconds() - 100), 1)
        }

        #[test]
        fn sync_filters_known_cosmetics_keeps_unknowns_and_preserves_revision() {
            use destiny_core::{
                DefinitionReplacementInput, DefinitionRepository, ItemDefinition, PlugSummary,
                Resolution, SyncMetadata,
            };
            struct Repository {
                inventory: InMemoryInventoryRepository,
                definitions: crate::test_support::InMemoryDefinitionRepository,
            }
            impl InventoryRepository for Repository {
                fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
                    self.inventory.load_current()
                }
                fn replace_current(
                    &mut self,
                    input: InventorySnapshotInput,
                ) -> Result<InventorySnapshot, AppError> {
                    self.inventory.replace_current(input)
                }
            }
            impl DefinitionRepository for Repository {
                fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
                    self.definitions.current_definition_version()
                }
                fn lookup_definition(
                    &self,
                    hash: ItemHash,
                ) -> Result<Resolution<ItemDefinition>, AppError> {
                    self.definitions.lookup_definition(hash)
                }
                fn current_metadata(&self) -> Result<Option<SyncMetadata>, AppError> {
                    self.definitions.current_metadata()
                }
                fn replace_definitions(
                    &mut self,
                    input: DefinitionReplacementInput,
                ) -> Result<SyncMetadata, AppError> {
                    self.definitions.replace_definitions(input)
                }
            }
            let definitions = [
                (60, Some("frames")),
                (61, Some("shader")),
                (63, None),
                (64, Some("shader")),
                (65, Some("enhancements.v2_head")),
                (66, Some("armor_archetypes")),
            ]
            .into_iter()
            .map(|(hash, category)| ItemDefinition {
                hash: ItemHash::new(hash),
                bucket_hash: None,
                name: "plug".into(),
                item_type: None,
                tier: None,
                element: None,
                slot: None,
                class_type: None,
                plug_category: category.map(str::to_owned),
                tags: vec![],
                version: DefinitionVersion::new(DEFINITION_VERSION),
            })
            .collect();
            let repository = Repository {
                inventory: InMemoryInventoryRepository::new(),
                definitions: crate::test_support::InMemoryDefinitionRepository::new(
                    DefinitionVersion::new(DEFINITION_VERSION),
                    definitions,
                ),
            };
            let resolved = membership();
            let shared = gateway();
            for cosmetic in [61, 64] {
                let mut candidate = inventory(&resolved, NOW, 1);
                candidate.items[0].available_plugs = [60, cosmetic, 65, 66, 62, 63]
                    .into_iter()
                    .map(|hash| destiny_core::AvailablePlug {
                        socket_index: 2,
                        plug_hash: ItemHash::new(hash),
                    })
                    .collect();
                candidate.items[0].plugs = [60, cosmetic, 65, 66, 62, 63, 60]
                    .into_iter()
                    .map(|hash| PlugSummary {
                        plug_hash: ItemHash::new(hash),
                        enabled: true,
                    })
                    .collect();
                shared.borrow_mut().queue_membership(Ok(resolved.clone()));
                shared.borrow_mut().queue_inventory(Ok(candidate));
            }
            let mut service =
                ProfileSyncService::new_with_definitions(SharedGateway(shared), repository);
            for _ in 0..2 {
                let outcome = service.sync(ProfileSyncReason::Forced, TTL, NOW).unwrap();
                assert_eq!(outcome.revision().value(), 1);
                let snapshot = service.repository().load_current().unwrap().unwrap();
                let hashes: Vec<_> = snapshot.items[0]
                    .plugs
                    .iter()
                    .map(|plug| plug.plug_hash.value())
                    .collect();
                // Known non-plugs (category None) are dropped, missing definitions kept.
                assert_eq!(hashes, [60, 65, 66, 62, 60]);
                assert_eq!(
                    snapshot.items[0]
                        .available_plugs
                        .iter()
                        .map(|plug| plug.plug_hash.value())
                        .collect::<Vec<_>>(),
                    [60, 65, 62]
                );
            }
        }

        #[test]
        fn plug_definition_lookup_failure_propagates_without_publication() {
            let resolved = membership();
            let shared = gateway();
            let mut candidate = inventory(&resolved, NOW, 1);
            candidate.items[0].plugs.push(destiny_core::PlugSummary {
                plug_hash: ItemHash::new(60),
                enabled: true,
            });
            shared.borrow_mut().queue_membership(Ok(resolved.clone()));
            shared.borrow_mut().queue_inventory(Ok(candidate));
            let mut service = harness(&shared, Some(fresh_seed(&resolved)));
            let before = committed_snapshot(&service);
            service.lookup_plug = |_, _| Err(AppError::new(ErrorCategory::LocalStorage));
            assert_eq!(
                service.sync(ProfileSyncReason::Forced, TTL, NOW),
                Err(AppError::new(ErrorCategory::LocalStorage))
            );
            assert_eq!(committed_snapshot(&service), before);
        }

        #[test]
        fn ordinary_without_a_committed_snapshot_refreshes() {
            let resolved = membership();
            let shared = gateway();
            shared.borrow_mut().queue_membership(Ok(resolved.clone()));
            shared
                .borrow_mut()
                .queue_inventory(Ok(inventory(&resolved, NOW, 1)));
            let mut service = harness(&shared, None);

            let outcome = service
                .sync(ProfileSyncReason::Ordinary, TTL, NOW)
                .expect("first sync succeeds");

            assert!(outcome.refreshed());
            assert_eq!(outcome.revision().value(), 1);

            let snapshot = committed_snapshot(&service);
            assert_eq!(snapshot.metadata.revision.value(), 1);
            assert_eq!(snapshot.metadata.checked_at, NOW);
            assert_eq!(snapshot.metadata.source, SourceMarker::Profile);
            assert_eq!(outcome.metadata(), &snapshot.metadata);
            assert_eq!(snapshot.metadata.membership_id, *resolved.membership_id());
            assert_eq!(
                snapshot.metadata.membership_type,
                resolved.membership_type()
            );

            let shared = shared.borrow();
            assert_eq!(shared.membership_calls(), 1);
            assert_eq!(shared.inventory_calls(), 1);
            assert_eq!(shared.requested_memberships(), [resolved]);
        }

        #[test]
        fn ordinary_over_a_fresh_snapshot_skips_without_calling_the_gateway() {
            let resolved = membership();
            let shared = gateway();
            let mut service = harness(&shared, Some(fresh_seed(&resolved)));
            let committed = committed_snapshot(&service);

            let outcome = service
                .sync(ProfileSyncReason::Ordinary, TTL, NOW)
                .expect("fresh skip succeeds");

            assert!(!outcome.refreshed());
            assert_eq!(outcome.revision(), committed.metadata.revision);
            assert_eq!(outcome.metadata(), &committed.metadata);
            assert_eq!(committed_snapshot(&service), committed);
            assert_eq!(
                service.repository().snapshot_metadata().unwrap(),
                Some(committed.metadata.clone())
            );

            // A repeated identical ordinary request is another no-op skip.
            let second = service
                .sync(ProfileSyncReason::Ordinary, TTL, NOW)
                .expect("second fresh skip succeeds");
            assert!(!second.refreshed());
            assert_eq!(second.metadata(), outcome.metadata());
            assert_eq!(committed_snapshot(&service), committed);

            let shared = shared.borrow();
            assert_eq!(shared.membership_calls(), 0);
            assert_eq!(shared.inventory_calls(), 0);
            assert!(shared.requested_memberships().is_empty());
        }

        #[test]
        fn ordinary_over_a_stale_snapshot_refreshes() {
            let resolved = membership();

            // Age strictly greater than the TTL.
            let shared = gateway();
            shared.borrow_mut().queue_membership(Ok(resolved.clone()));
            shared
                .borrow_mut()
                .queue_inventory(Ok(inventory(&resolved, NOW, 1)));
            let aged = Timestamp::new(NOW.seconds() - TTL.ttl_seconds - 1);
            let mut service = harness(&shared, Some(inventory(&resolved, aged, 1)));

            let refreshed = service
                .sync(ProfileSyncReason::Ordinary, TTL, NOW)
                .expect("stale refresh succeeds");
            assert!(refreshed.refreshed());
            assert_eq!(committed_snapshot(&service).metadata.checked_at, NOW);
            assert_eq!(shared.borrow().membership_calls(), 1);

            // A committed timestamp in the future is stale, not fresh.
            let shared = gateway();
            shared.borrow_mut().queue_membership(Ok(resolved.clone()));
            shared
                .borrow_mut()
                .queue_inventory(Ok(inventory(&resolved, NOW, 1)));
            let future = Timestamp::new(NOW.seconds() + 1);
            let mut service = harness(&shared, Some(inventory(&resolved, future, 1)));

            let refreshed = service
                .sync(ProfileSyncReason::Ordinary, TTL, NOW)
                .expect("future-stamped refresh succeeds");
            assert!(refreshed.refreshed());
            assert_eq!(committed_snapshot(&service).metadata.checked_at, NOW);
            assert_eq!(shared.borrow().membership_calls(), 1);
        }

        #[test]
        fn ordinary_over_a_missing_snapshot_refreshes() {
            let resolved = membership();
            let shared = gateway();
            shared.borrow_mut().queue_membership(Ok(resolved.clone()));
            shared
                .borrow_mut()
                .queue_inventory(Ok(inventory(&resolved, NOW, 1)));
            let mut service = harness(&shared, None);

            let outcome = service
                .sync(ProfileSyncReason::Ordinary, TTL, NOW)
                .expect("missing refresh succeeds");

            assert!(outcome.refreshed());
            assert_eq!(outcome.revision().value(), 1);
        }

        #[test]
        fn ordinary_over_an_invalid_source_snapshot_refreshes() {
            let resolved = membership();
            let mut mis_sourced = inventory(&resolved, NOW, 1);
            mis_sourced.source = SourceMarker::Manifest;
            let repository = InMemoryInventoryRepository::with_revision_for_test(
                mis_sourced,
                SyncRevision::new(4),
            );

            let shared = gateway();
            shared.borrow_mut().queue_membership(Ok(resolved.clone()));
            shared
                .borrow_mut()
                .queue_inventory(Ok(inventory(&resolved, NOW, 1)));
            let mut service =
                ProfileSyncService::new(SharedGateway(Rc::clone(&shared)), repository);

            let outcome = service
                .sync(ProfileSyncReason::Ordinary, TTL, NOW)
                .expect("mis-sourced refresh succeeds");

            // The projection refreshes even though the content is unchanged,
            // because the committed provenance marker was not trustworthy.
            assert!(outcome.refreshed());
            let snapshot = committed_snapshot(&service);
            assert_eq!(snapshot.metadata.source, SourceMarker::Profile);
            assert_eq!(snapshot.metadata.revision.value(), 4);
            assert_eq!(shared.borrow().membership_calls(), 1);
        }

        #[test]
        fn every_non_ordinary_reason_refreshes_a_fresh_snapshot() {
            for reason in [
                ProfileSyncReason::Forced,
                ProfileSyncReason::Preflight,
                ProfileSyncReason::PostAction,
            ] {
                let resolved = membership();
                let shared = gateway();
                shared.borrow_mut().queue_membership(Ok(resolved.clone()));
                // Unchanged content at a later timestamp: the revision is
                // retained while committed `checked_at` refreshes.
                let later = Timestamp::new(NOW.seconds() - 10);
                shared
                    .borrow_mut()
                    .queue_inventory(Ok(inventory(&resolved, later, 1)));
                let mut service = harness(&shared, Some(fresh_seed(&resolved)));

                let outcome = service
                    .sync(reason, TTL, NOW)
                    .unwrap_or_else(|_| panic!("{reason:?} refresh succeeds"));

                assert!(outcome.refreshed(), "{reason:?}");
                let snapshot = committed_snapshot(&service);
                assert_eq!(snapshot.metadata.revision.value(), 1, "{reason:?}");
                assert_eq!(snapshot.metadata.checked_at, later, "{reason:?}");

                let shared = shared.borrow();
                assert_eq!(shared.membership_calls(), 1, "{reason:?}");
                assert_eq!(shared.inventory_calls(), 1, "{reason:?}");
            }
        }

        #[test]
        fn membership_identifier_mismatch_is_rejected_before_publication() {
            let resolved = membership();
            let other = membership_with("1234567890123456789", 3);
            let shared = gateway();
            shared.borrow_mut().queue_membership(Ok(resolved.clone()));
            shared
                .borrow_mut()
                .queue_inventory(Ok(inventory(&other, NOW, 1)));
            let mut service = harness(&shared, Some(fresh_seed(&resolved)));
            let prior = committed_snapshot(&service);

            let error = service
                .sync(ProfileSyncReason::Forced, TTL, NOW)
                .expect_err("membership mismatch must fail");

            assert_eq!(error.category(), ErrorCategory::ProviderValidation);
            // The mismatched candidate never replaced the prior projection.
            assert_eq!(committed_snapshot(&service), prior);
            assert_eq!(
                committed_snapshot(&service).metadata.membership_id,
                *resolved.membership_id()
            );
            assert_eq!(shared.borrow().membership_calls(), 1);
            assert_eq!(shared.borrow().inventory_calls(), 1);
        }

        #[test]
        fn membership_type_mismatch_is_rejected_before_publication() {
            let resolved = membership();
            let wrong_type = membership_with("4611686018484639825", 254);
            let shared = gateway();
            shared.borrow_mut().queue_membership(Ok(resolved.clone()));
            shared
                .borrow_mut()
                .queue_inventory(Ok(inventory(&wrong_type, NOW, 1)));
            let mut service = harness(&shared, Some(fresh_seed(&resolved)));
            let prior = committed_snapshot(&service);

            let error = service
                .sync(ProfileSyncReason::Forced, TTL, NOW)
                .expect_err("membership-type mismatch must fail");

            assert_eq!(error.category(), ErrorCategory::ProviderValidation);
            assert_eq!(committed_snapshot(&service), prior);
        }

        #[test]
        fn resolve_failure_propagates_and_preserves_prior_state() {
            let resolved = membership();
            let shared = gateway();
            shared
                .borrow_mut()
                .queue_membership(Err(AppError::new(ErrorCategory::ProviderUnavailable)));
            let mut service = harness(&shared, Some(fresh_seed(&resolved)));
            let prior = committed_snapshot(&service);

            let error = service
                .sync(ProfileSyncReason::Forced, TTL, NOW)
                .expect_err("resolve failure must fail");

            assert_eq!(error.category(), ErrorCategory::ProviderUnavailable);
            assert_eq!(committed_snapshot(&service), prior);
            assert_eq!(shared.borrow().membership_calls(), 1);
            assert_eq!(shared.borrow().inventory_calls(), 0);
        }

        #[test]
        fn load_failure_propagates_and_preserves_prior_state() {
            let resolved = membership();
            let shared = gateway();
            shared.borrow_mut().queue_membership(Ok(resolved.clone()));
            shared
                .borrow_mut()
                .queue_inventory(Err(AppError::new(ErrorCategory::ProviderRateLimit)));
            let mut service = harness(&shared, Some(fresh_seed(&resolved)));
            let prior = committed_snapshot(&service);

            let error = service
                .sync(ProfileSyncReason::Forced, TTL, NOW)
                .expect_err("load failure must fail");

            assert_eq!(error.category(), ErrorCategory::ProviderRateLimit);
            assert_eq!(committed_snapshot(&service), prior);
            assert_eq!(shared.borrow().inventory_calls(), 1);
        }

        #[test]
        fn repository_rejection_preserves_prior_state() {
            let resolved = membership();

            // Wrong provenance marker.
            let shared = gateway();
            shared.borrow_mut().queue_membership(Ok(resolved.clone()));
            let mut wrong_source = inventory(&resolved, NOW, 1);
            wrong_source.source = SourceMarker::Manifest;
            shared.borrow_mut().queue_inventory(Ok(wrong_source));
            let mut service = harness(&shared, Some(fresh_seed(&resolved)));
            let prior = committed_snapshot(&service);
            assert_eq!(
                service
                    .sync(ProfileSyncReason::Forced, TTL, NOW)
                    .expect_err("invalid source must fail")
                    .category(),
                ErrorCategory::ProviderValidation
            );
            assert_eq!(committed_snapshot(&service), prior);

            // Structurally invalid: a dangling character location.
            let shared = gateway();
            shared.borrow_mut().queue_membership(Ok(resolved.clone()));
            let mut dangling = inventory(&resolved, NOW, 1);
            dangling.items[0].location = ItemLocation::Character("missing-character".into());
            shared.borrow_mut().queue_inventory(Ok(dangling));
            let mut service = harness(&shared, Some(fresh_seed(&resolved)));
            let prior = committed_snapshot(&service);
            assert_eq!(
                service
                    .sync(ProfileSyncReason::Forced, TTL, NOW)
                    .expect_err("structurally invalid candidate must fail")
                    .category(),
                ErrorCategory::ProviderValidation
            );
            assert_eq!(committed_snapshot(&service), prior);
        }

        #[test]
        fn unchanged_candidate_retains_revision_and_changed_candidate_advances_once() {
            let resolved = membership();
            let later = Timestamp::new(NOW.seconds() - 10);
            let shared = gateway();
            shared.borrow_mut().queue_membership(Ok(resolved.clone()));
            shared
                .borrow_mut()
                .queue_inventory(Ok(inventory(&resolved, later, 1)));
            shared.borrow_mut().queue_membership(Ok(resolved.clone()));
            shared
                .borrow_mut()
                .queue_inventory(Ok(inventory(&resolved, NOW, 2)));
            let mut service = harness(&shared, Some(fresh_seed(&resolved)));

            let first = service
                .sync(ProfileSyncReason::Forced, TTL, NOW)
                .expect("unchanged refresh succeeds");
            assert_eq!(first.revision().value(), 1);
            assert_eq!(committed_snapshot(&service).metadata.checked_at, later);

            let second = service
                .sync(ProfileSyncReason::Forced, TTL, NOW)
                .expect("changed refresh succeeds");
            assert_eq!(second.revision().value(), 2);
            assert_eq!(second.revision().value(), first.revision().value() + 1);
            assert_eq!(shared.borrow().inventory_calls(), 2);
        }

        #[test]
        fn age_equal_to_ttl_is_fresh_and_a_future_stamp_is_stale() {
            let resolved = membership();

            // Exactly at the boundary: Fresh, so an Ordinary request skips.
            let shared = gateway();
            let boundary = Timestamp::new(NOW.seconds() - TTL.ttl_seconds);
            let mut service = harness(&shared, Some(inventory(&resolved, boundary, 1)));
            let skipped = service
                .sync(ProfileSyncReason::Ordinary, TTL, NOW)
                .expect("boundary skip succeeds");
            assert!(!skipped.refreshed());
            assert_eq!(shared.borrow().membership_calls(), 0);

            // One second beyond the boundary: Stale, so it refreshes.
            let shared = gateway();
            shared.borrow_mut().queue_membership(Ok(resolved.clone()));
            shared
                .borrow_mut()
                .queue_inventory(Ok(inventory(&resolved, NOW, 1)));
            let just_stale = Timestamp::new(NOW.seconds() - TTL.ttl_seconds - 1);
            let mut service = harness(&shared, Some(inventory(&resolved, just_stale, 1)));
            assert!(
                service
                    .sync(ProfileSyncReason::Ordinary, TTL, NOW)
                    .expect("just-stale refresh succeeds")
                    .refreshed()
            );
            assert_eq!(shared.borrow().membership_calls(), 1);
        }

        #[test]
        fn failures_render_without_membership_identifiers_or_payloads() {
            const IDENTIFIER: &str = "4611686018484639825";
            const PAYLOAD: &str = "raw-provider-payload-sentinel";

            let resolved = membership_with(IDENTIFIER, 3);
            let mut candidate = inventory(&resolved, NOW, 1);
            candidate.items[0].instance_id = PAYLOAD.into();
            candidate.membership_id = MembershipId::new("9999999999999999999");

            let shared = gateway();
            shared.borrow_mut().queue_membership(Ok(resolved.clone()));
            shared.borrow_mut().queue_inventory(Ok(candidate));
            let mut service = harness(&shared, None);

            let error = service
                .sync(ProfileSyncReason::Ordinary, TTL, NOW)
                .expect_err("mismatch must fail");

            assert_eq!(error.category(), ErrorCategory::ProviderValidation);
            for rendered in [error.to_string(), format!("{error:?}")] {
                assert!(
                    !rendered.contains(IDENTIFIER),
                    "leaked membership identifier: {rendered}"
                );
                assert!(!rendered.contains(PAYLOAD), "leaked payload: {rendered}");
            }
        }
    }
}
