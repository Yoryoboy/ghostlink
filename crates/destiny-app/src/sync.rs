//! Offline profile synchronization orchestration.
//!
//! The service is provider- and transport-neutral. It reads the committed
//! projection through the core-owned `InventoryRepository`, asks the core-owned
//! `ProfileGateway` for one complete normalized candidate when a refresh is
//! warranted, verifies that the candidate belongs to the membership it resolved,
//! and commits it atomically. It never retains or surfaces raw payloads, upstream
//! messages, or credential material. Failures stay sanitized [`AppError`]
//! categories without membership identifiers; successful snapshots retain identity.
//!
//! The caller supplies all policy input: the sync `reason`, the freshness
//! `policy`, and the explicit `now` timestamp. The service reads no ambient
//! clock. It never stamps `checked_at`, `source`, or `definition_version`, and
//! never computes a revision: the gateway stamps acquisition metadata and the
//! repository owns provenance, structural validation, and revision allocation.

use destiny_core::{
    AppError, Dataset, ErrorCategory, FreshnessInput, FreshnessPolicy, FreshnessStatus,
    InstancedBucketObservation, InventoryRepository, InventorySnapshot, InventorySnapshotInput,
    ProfileAcquisition, ProfileGateway, ProfileSyncOutcome, ProfileSyncReason, SnapshotMetadata,
    SourceMarker, Timestamp,
};

/// A successful publication return paired with its ephemeral acquisition observations.
///
/// This association does NOT establish authentication, causality, capacity, action
/// authority, durability of runtime buckets, or perpetual freshness. Planning AND
/// execution must reacquire; an unchanged revision cannot authorize proof reuse.
/// `None` means unavailable; `Some([])` means available and complete with zero instances.
/// Only sync can construct this type, after validating the exact publication return.
///
/// ```compile_fail
/// use destiny_app::sync::PublishedProfileAcquisition;
/// fn mutate(pair: &mut PublishedProfileAcquisition) {
///     pair.snapshot().items.clear();
/// }
/// ```
/// ```compile_fail
/// use destiny_app::{InventorySnapshot, InventorySnapshotInput};
/// use destiny_app::sync::PublishedProfileAcquisition;
/// fn fabricate(candidate: &InventorySnapshotInput, snapshot: InventorySnapshot) {
///     let _ = PublishedProfileAcquisition::new(candidate, snapshot, None);
/// }
/// ```
#[derive(Debug, Eq, PartialEq)]
pub struct PublishedProfileAcquisition {
    snapshot: InventorySnapshot,
    observations: Option<Vec<InstancedBucketObservation>>,
}

impl PublishedProfileAcquisition {
    fn new(
        candidate: &InventorySnapshotInput,
        snapshot: InventorySnapshot,
        observations: Option<Vec<InstancedBucketObservation>>,
    ) -> Result<Self, AppError> {
        let returned = InventorySnapshotInput::with_metadata(&snapshot);
        // content_eq deliberately excludes time and source. Check both exactly,
        // and require the dataset's source independently of candidate equality.
        if !returned.is_structurally_valid()
            || !candidate.content_eq(&returned)
            || candidate.checked_at != returned.checked_at
            || candidate.source != returned.source
            || returned.source != SourceMarker::Profile
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        let (_, observations) = ProfileAcquisition::new(returned, observations)?.into_parts();
        Ok(Self {
            snapshot,
            observations,
        })
    }

    pub fn snapshot(&self) -> &InventorySnapshot {
        &self.snapshot
    }

    pub fn observations(&self) -> Option<&[InstancedBucketObservation]> {
        self.observations.as_deref()
    }

    /// Consumes and explicitly ends pairing; neither part grants action authority.
    pub fn into_parts(self) -> (InventorySnapshot, Option<Vec<InstancedBucketObservation>>) {
        (self.snapshot, self.observations)
    }
}

/// Non-publications carry metadata, never a fabricated observation pair.
/// `discarded` distinguishes an Ordinary not-newer acquisition from a fresh
/// skip. On discard the retained metadata may still be stale.
#[derive(Debug, Eq, PartialEq)]
pub enum ProfileSyncWithObservationsOutcome {
    SkippedFresh {
        metadata: SnapshotMetadata,
        discarded: bool,
    },
    Refreshed(PublishedProfileAcquisition),
}

// Ordinary publication intentionally does not construct or validate a core pair:
// the legacy loader/filter/lookup-error ordering is part of its existing contract.
enum SyncPublication {
    Legacy(InventorySnapshot),
    Observed(PublishedProfileAcquisition),
}
enum SyncDecision {
    SkippedFresh(SnapshotMetadata),
    DiscardedNotNewer(SnapshotMetadata),
    Refreshed(SyncPublication),
}

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
    sync_gate: Option<std::sync::Arc<std::sync::Mutex<()>>>,
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
            sync_gate: None,
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
            sync_gate: None,
            lookup_plug: |repository, hash| repository.lookup_definition(hash),
        }
    }

    /// Serializes read-triggered refreshes with the caller's mutation boundary.
    /// Fresh Ordinary reads bypass the gate. Waiters re-check freshness under it.
    /// Never inject this into a service called while already holding the gate.
    /// Lock order: read service -> gate -> individual repository/gateway calls;
    /// repository and gateway locks must be released before acquiring the gate.
    pub fn with_sync_gate(mut self, gate: std::sync::Arc<std::sync::Mutex<()>>) -> Self {
        self.sync_gate = Some(gate);
        self
    }

    /// Transfers the authenticated gateway and committed repository together.
    pub fn into_parts(self) -> (G, R) {
        (self.gateway, self.repository)
    }

    /// Shares the authenticated gateway with sibling live-only reads.
    pub(crate) fn gateway_mut(&mut self) -> &mut G {
        &mut self.gateway
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
    /// calling the gateway or publishing a replacement. Otherwise the gateway
    /// resolves the membership and loads one complete candidate, the candidate
    /// is verified against that membership, and the verified candidate is
    /// committed through the repository, which owns revision retention and
    /// advancement. The outcome carries repository metadata only.
    ///
    /// An Ordinary or Forced same-membership candidate whose minted instant is
    /// not newer is discarded before filtering/publication: Bungie can serve a
    /// stale replica for minutes after an action, and publishing it would move
    /// the projection back in time. The caller may use the retained projection
    /// for this call, but its freshness and invalidation are unchanged;
    /// subsequent calls can acquire again. Preflight and post-action
    /// observations still publish until their callers adopt local sync.
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
        match self.sync_inner(reason, policy, now, false)? {
            SyncDecision::SkippedFresh(metadata) => {
                Ok(ProfileSyncOutcome::SkippedFresh { metadata })
            }
            SyncDecision::DiscardedNotNewer(metadata) => {
                Ok(ProfileSyncOutcome::DiscardedNotNewer { metadata })
            }
            SyncDecision::Refreshed(SyncPublication::Legacy(snapshot)) => {
                Ok(ProfileSyncOutcome::Refreshed {
                    metadata: snapshot.metadata,
                })
            }
            SyncDecision::Refreshed(SyncPublication::Observed(_)) => unreachable!("legacy route"),
        }
    }

    /// Acquires ephemeral observations and releases them only after successful
    /// publication, associated with exactly the returned snapshot. An Ordinary
    /// or Forced discard returns the skip shape with `discarded: true`, without
    /// observations. Preflight/PostAction cannot return that discard marker. No reload,
    /// retry, bucket persistence, or revision-based authority is introduced.
    /// A mismatched successful repository return is rejected, not rolled back.
    pub fn sync_with_observations(
        &mut self,
        reason: ProfileSyncReason,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<ProfileSyncWithObservationsOutcome, AppError> {
        match self.sync_inner(reason, policy, now, true)? {
            SyncDecision::SkippedFresh(metadata) => {
                Ok(ProfileSyncWithObservationsOutcome::SkippedFresh {
                    metadata,
                    discarded: false,
                })
            }
            SyncDecision::DiscardedNotNewer(metadata) => {
                Ok(ProfileSyncWithObservationsOutcome::SkippedFresh {
                    metadata,
                    discarded: true,
                })
            }
            SyncDecision::Refreshed(SyncPublication::Observed(pair)) => {
                Ok(ProfileSyncWithObservationsOutcome::Refreshed(pair))
            }
            SyncDecision::Refreshed(SyncPublication::Legacy(_)) => unreachable!("observed route"),
        }
    }

    fn sync_inner(
        &mut self,
        reason: ProfileSyncReason,
        policy: FreshnessPolicy,
        now: Timestamp,
        with_observations: bool,
    ) -> Result<SyncDecision, AppError> {
        self.sync_inner_gated(reason, policy, now, with_observations, false)
    }

    fn sync_inner_gated(
        &mut self,
        reason: ProfileSyncReason,
        policy: FreshnessPolicy,
        now: Timestamp,
        with_observations: bool,
        gate_held: bool,
    ) -> Result<SyncDecision, AppError> {
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

        if reason == ProfileSyncReason::Ordinary
            && status == FreshnessStatus::Fresh
            && !self.repository.freshness_invalidated()?
        {
            // `Fresh` requires a committed `checked_at`, so a fresh status
            // implies the snapshot is present; moving its metadata avoids
            // cloning the projection. No gateway call and no mutation happen.
            if let Some(snapshot) = committed {
                return Ok(SyncDecision::SkippedFresh(snapshot.metadata));
            }
        }

        if !gate_held && let Some(gate) = self.sync_gate.clone() {
            let _exclusive = gate
                .lock()
                .map_err(|_| AppError::new(ErrorCategory::LocalStorage))?;
            // No transaction/adapter lock survives the first check. Re-check
            // because another refresher may have published while we waited.
            return self.sync_inner_gated(reason, policy, now, with_observations, true);
        }

        let membership = self.gateway.resolve_membership_scoped(reason)?;
        let (mut candidate, observations, minted, loadouts) = if with_observations {
            let pair = self
                .gateway
                .load_inventory_with_observations_scoped(&membership, reason)?;
            let minted = pair.minted_at().copied();
            let loadouts = pair.saved_loadouts().map(<[_]>::to_vec);
            let (input, observations) = pair.into_parts();
            (input, observations, minted, loadouts)
        } else {
            let (input, minted, loadouts) = self
                .gateway
                .load_inventory_profile_scoped(&membership, reason)?;
            (input, None, minted, loadouts)
        };
        // The candidate must belong to the membership it was resolved for, so a
        // mismatched or partial provider response can never be published.
        if candidate.membership_id != *membership.membership_id()
            || candidate.membership_type != membership.membership_type()
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        // Ordinary reads and explicit syncs keep locally acknowledged effects
        // when Bungie's profile lags. Do not look up definitions, publish
        // observations, clear invalidation or advance checked_at for a discarded
        // acquisition. Preflight/post-action routes intentionally retain
        // publication until the remaining mutation pipelines adopt local sync.
        if matches!(
            reason,
            ProfileSyncReason::Ordinary | ProfileSyncReason::Forced
        ) && let Some(prior) = committed
            && prior.metadata.membership_id == candidate.membership_id
            && prior.metadata.membership_type == candidate.membership_type
            && let (Some(next), Some(previous)) = (minted, self.repository.profile_minted_at()?)
            && next <= previous
        {
            return Ok(SyncDecision::DiscardedNotNewer(prior.metadata));
        }
        // Resolve each distinct plug once before publication. Lookup failures
        // abort without writing; missing or ambiguous definitions are unknown
        // and retained. A known definition without a category is not a perk.
        let mut categories = std::collections::HashMap::new();
        let mut subclass_options = std::collections::HashSet::new();
        for hash in candidate
            .items
            .iter()
            .flat_map(|item| {
                item.plugs
                    .iter()
                    .map(|plug| plug.plug_hash)
                    .chain(item.available_plugs.iter().map(|plug| plug.plug_hash))
            })
            .chain(candidate.plug_set_entries.iter().map(|plug| plug.plug_hash))
        {
            if let std::collections::hash_map::Entry::Vacant(entry) = categories.entry(hash) {
                let category = match (self.lookup_plug)(&self.repository, hash)? {
                    destiny_core::Resolution::Unique(definition) => {
                        if definition
                            .plug_category
                            .as_deref()
                            .and_then(destiny_core::classify_subclass_plug)
                            .is_some()
                            && !(definition.name.starts_with("Empty ")
                                && definition.name.ends_with(" Socket"))
                        {
                            subclass_options.insert(hash);
                        }
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
        // Unlike item plugs, unknown plug-set definitions are dropped: live
        // profiles contain ~924 sets, mostly cosmetics, not subclass options.
        candidate
            .plug_set_entries
            .retain(|entry| subclass_options.contains(&entry.plug_hash));
        // Preserve only the observed route's POST-FILTER candidate. Ordinary
        // success still moves its input without cloning a projection.
        let expected = with_observations.then(|| candidate.clone());
        let snapshot = self
            .repository
            .replace_current_profile(candidate, minted, loadouts)?;
        let publication = match expected {
            Some(candidate) => SyncPublication::Observed(PublishedProfileAcquisition::new(
                &candidate,
                snapshot,
                observations,
            )?),
            None => SyncPublication::Legacy(snapshot),
        };
        Ok(SyncDecision::Refreshed(publication))
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

        #[test]
        fn concurrent_stale_reads_acquire_once_and_wait_for_mutation() {
            use std::sync::{
                Arc, Barrier, Mutex,
                atomic::{AtomicBool, AtomicUsize, Ordering},
                mpsc,
            };
            use std::time::Duration;
            struct Store {
                inner: Arc<Mutex<InMemoryInventoryRepository>>,
                first: AtomicBool,
                checked: Arc<Barrier>,
                ready: mpsc::Sender<()>,
            }
            impl InventoryRepository for Store {
                fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
                    let current = self.inner.lock().unwrap().load_current()?;
                    if self.first.swap(false, Ordering::SeqCst) {
                        self.checked.wait();
                        self.ready.send(()).unwrap();
                    }
                    Ok(current)
                }
                fn replace_current(
                    &mut self,
                    input: InventorySnapshotInput,
                ) -> Result<InventorySnapshot, AppError> {
                    self.inner.lock().unwrap().replace_current(input)
                }
            }
            struct Gateway {
                calls: Arc<AtomicUsize>,
                mutation_active: Arc<AtomicBool>,
                exclusive: Arc<Mutex<()>>,
            }
            impl ProfileGateway for Gateway {
                fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
                    Ok(membership())
                }
                fn load_inventory(
                    &mut self,
                    _: &DestinyMembership,
                ) -> Result<InventorySnapshotInput, AppError> {
                    assert!(
                        !self.mutation_active.load(Ordering::SeqCst),
                        "sync interleaved with mutation"
                    );
                    assert!(
                        matches!(
                            self.exclusive.try_lock(),
                            Err(std::sync::TryLockError::WouldBlock)
                        ),
                        "profile acquisition must hold the mutation guard"
                    );
                    self.calls.fetch_add(1, Ordering::SeqCst);
                    Ok(inventory(&membership(), NOW, 1))
                }
            }
            for _ in 0..20 {
                let store = Arc::new(Mutex::new(InMemoryInventoryRepository::new()));
                let gate = Arc::new(Mutex::new(()));
                let mutation = gate.lock().unwrap();
                let active = Arc::new(AtomicBool::new(true));
                let calls = Arc::new(AtomicUsize::new(0));
                let checked = Arc::new(Barrier::new(2));
                let (ready_tx, ready_rx) = mpsc::channel();
                let workers: Vec<_> = (0..2)
                    .map(|_| {
                        let mut service = ProfileSyncService::new(
                            Gateway {
                                calls: calls.clone(),
                                mutation_active: active.clone(),
                                exclusive: gate.clone(),
                            },
                            Store {
                                inner: store.clone(),
                                first: AtomicBool::new(true),
                                checked: checked.clone(),
                                ready: ready_tx.clone(),
                            },
                        )
                        .with_sync_gate(gate.clone());
                        std::thread::spawn(move || {
                            service.sync(ProfileSyncReason::Ordinary, TTL, NOW)
                        })
                    })
                    .collect();
                for _ in 0..2 {
                    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                active.store(false, Ordering::SeqCst);
                drop(mutation);
                for worker in workers {
                    assert!(worker.join().unwrap().unwrap().metadata().checked_at == NOW);
                }
                assert_eq!(
                    calls.load(Ordering::SeqCst),
                    1,
                    "duplicate profile acquisition"
                );
            }
        }

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
                profile_currencies_coverage: destiny_core::ProfileCurrenciesCoverage::NotObserved,
                stacks: Vec::new(),
                plug_set_entries: Vec::new(),
                membership_id: membership.membership_id().clone(),
                membership_type: membership.membership_type(),
                definition_version: DefinitionVersion::new(DEFINITION_VERSION),
                checked_at,
                source: SourceMarker::Profile,
                characters: Vec::new(),
                items: vec![ItemInstance {
                    energy: None,
                    gear_tier: None,
                    sockets: Vec::new(),
                    transfer_status: None,
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

        struct MintedGateway(destiny_core::ProfileAcquisition);
        impl ProfileGateway for MintedGateway {
            fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
                let input = self.0.input();
                DestinyMembership::new(input.membership_type, input.membership_id.clone())
            }
            fn load_inventory(
                &mut self,
                _: &DestinyMembership,
            ) -> Result<InventorySnapshotInput, AppError> {
                Ok(self.0.input().clone())
            }
            fn load_inventory_minted_scoped(
                &mut self,
                _: &DestinyMembership,
                _: ProfileSyncReason,
            ) -> Result<
                (
                    InventorySnapshotInput,
                    Option<destiny_core::ProfileMintedAt>,
                ),
                AppError,
            > {
                Ok((self.0.input().clone(), self.0.minted_at().copied()))
            }
            fn load_inventory_with_observations_scoped(
                &mut self,
                _: &DestinyMembership,
                _: ProfileSyncReason,
            ) -> Result<destiny_core::ProfileAcquisition, AppError> {
                Ok(self.0.clone())
            }
        }

        #[test]
        fn minted_guard_preserves_effects_on_ordinary_and_forced_but_not_preflight() {
            use destiny_core::{InventoryEffect, ProfileMintedAt};
            for observed in [false, true] {
                for invalidated in [false, true] {
                    for (prior_minted, next_minted, switch, reason, discarded) in [
                        (
                            Some(200),
                            Some(199),
                            false,
                            ProfileSyncReason::Ordinary,
                            true,
                        ),
                        (
                            Some(200),
                            Some(200),
                            false,
                            ProfileSyncReason::Ordinary,
                            true,
                        ),
                        (
                            Some(200),
                            Some(201),
                            false,
                            ProfileSyncReason::Ordinary,
                            false,
                        ),
                        (None, Some(199), false, ProfileSyncReason::Ordinary, false),
                        (Some(200), None, false, ProfileSyncReason::Ordinary, false),
                        (
                            Some(200),
                            Some(199),
                            true,
                            ProfileSyncReason::Ordinary,
                            false,
                        ),
                        (
                            Some(200),
                            Some(199),
                            false,
                            ProfileSyncReason::Preflight,
                            false,
                        ),
                        (
                            Some(200),
                            Some(200),
                            false,
                            ProfileSyncReason::Preflight,
                            false,
                        ),
                        // An explicit sync never moves the projection back in time:
                        // a replica minted no later than the stored one is discarded.
                        (Some(200), Some(199), false, ProfileSyncReason::Forced, true),
                        (Some(200), Some(200), false, ProfileSyncReason::Forced, true),
                        (
                            Some(200),
                            Some(201),
                            false,
                            ProfileSyncReason::Forced,
                            false,
                        ),
                        (None, Some(199), false, ProfileSyncReason::Forced, false),
                        (Some(200), Some(199), true, ProfileSyncReason::Forced, false),
                    ] {
                        let instant = |n| ProfileMintedAt::new(1_800_000_000, n).unwrap();
                        let mut repo = InMemoryInventoryRepository::new();
                        repo.replace_current_minted(
                            inventory(&membership(), Timestamp::new(1), 1),
                            prior_minted.map(instant),
                        )
                        .unwrap();
                        let effected = repo
                            .apply_acknowledged_effects(
                                &membership(),
                                &[InventoryEffect::SetLocked {
                                    item: "instance-1".into(),
                                    locked: true,
                                }],
                            )
                            .unwrap();
                        if invalidated {
                            repo.invalidate_freshness().unwrap();
                        }
                        let target = if switch {
                            membership_with("4611686018484639000", 2)
                        } else {
                            membership()
                        };
                        let input = inventory(&target, NOW, 2);
                        let pair = acquisition(input, 99).with_minted_at(next_minted.map(instant));
                        let mut service = ProfileSyncService::new(MintedGateway(pair), repo);
                        if observed {
                            let outcome = service.sync_with_observations(reason, TTL, NOW).unwrap();
                            if discarded {
                                assert!(matches!(outcome, crate::sync::ProfileSyncWithObservationsOutcome::SkippedFresh { discarded: true, .. }), "discard must not release observations");
                            } else {
                                assert!(matches!(
                                    outcome,
                                    crate::sync::ProfileSyncWithObservationsOutcome::Refreshed(_)
                                ));
                            }
                        } else {
                            let outcome = service.sync(reason, TTL, NOW).unwrap();
                            assert_eq!(outcome.refreshed(), !discarded);
                            assert_eq!(
                                matches!(
                                    outcome,
                                    destiny_core::ProfileSyncOutcome::DiscardedNotNewer { .. }
                                ),
                                discarded
                            );
                        }
                        let repo = service.repository();
                        let after = repo.load_current().unwrap().unwrap();
                        if discarded {
                            assert_eq!(after, effected);
                            assert_eq!(
                                repo.profile_minted_at().unwrap(),
                                prior_minted.map(instant)
                            );
                            assert_eq!(repo.freshness_invalidated().unwrap(), invalidated);
                            // No TTL advancement: another stale ordinary call fetches and
                            // discards again, rather than treating retained data as fresh.
                            let outcome = service
                                .sync_with_observations(ProfileSyncReason::Ordinary, TTL, NOW)
                                .unwrap();
                            assert!(matches!(
                                outcome,
                                crate::sync::ProfileSyncWithObservationsOutcome::SkippedFresh { .. }
                            ));
                        } else {
                            assert_eq!(after.items[0].quantity, 2);
                            assert!(!after.items[0].locked);
                            assert_eq!(after.metadata.checked_at, NOW);
                            assert_eq!(repo.profile_minted_at().unwrap(), next_minted.map(instant));
                            assert!(!repo.freshness_invalidated().unwrap());
                        }
                    }
                }
            }
        }

        // This seam supplies validated acquisitions, never bypassing the core pair.
        struct ObservationGateway {
            acquisition: Option<destiny_core::ProfileAcquisition>,
            calls: Rc<RefCell<Vec<ProfileSyncReason>>>,
        }

        impl ProfileGateway for ObservationGateway {
            fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
                panic!("scoped resolver required")
            }
            fn load_inventory(
                &mut self,
                _: &DestinyMembership,
            ) -> Result<InventorySnapshotInput, AppError> {
                panic!("observation route must not call legacy loader")
            }
            fn resolve_membership_scoped(
                &mut self,
                reason: ProfileSyncReason,
            ) -> Result<DestinyMembership, AppError> {
                self.calls.borrow_mut().push(reason);
                Ok(membership())
            }
            fn load_inventory_with_observations_scoped(
                &mut self,
                requested: &DestinyMembership,
                reason: ProfileSyncReason,
            ) -> Result<destiny_core::ProfileAcquisition, AppError> {
                assert_eq!(requested, &membership());
                self.calls.borrow_mut().push(reason);
                Ok(self.acquisition.take().expect("one acquisition"))
            }
        }

        fn acquisition(
            input: InventorySnapshotInput,
            bucket: i64,
        ) -> destiny_core::ProfileAcquisition {
            let rows = input
                .items
                .iter()
                .map(|item| {
                    destiny_core::InstancedBucketObservation::new(
                        item.instance_id.clone(),
                        destiny_core::BucketHash::new(bucket),
                        item.location.clone(),
                    )
                    .unwrap()
                })
                .collect();
            destiny_core::ProfileAcquisition::new(input, Some(rows)).unwrap()
        }

        fn observed_gateway(
            input: InventorySnapshotInput,
        ) -> (ObservationGateway, Rc<RefCell<Vec<ProfileSyncReason>>>) {
            let calls = Rc::new(RefCell::new(Vec::new()));
            (
                ObservationGateway {
                    acquisition: Some(acquisition(input, 99)),
                    calls: Rc::clone(&calls),
                },
                calls,
            )
        }

        #[test]
        fn observations_skip_is_not_an_unavailable_publication() {
            use crate::sync::ProfileSyncWithObservationsOutcome;
            let shared = gateway();
            let mut service = harness(&shared, Some(fresh_seed(&membership())));
            assert!(matches!(
                service
                    .sync_with_observations(ProfileSyncReason::Ordinary, TTL, NOW)
                    .unwrap(),
                ProfileSyncWithObservationsOutcome::SkippedFresh { .. }
            ));
            assert_eq!(shared.borrow().membership_calls(), 0);
            assert_eq!(shared.borrow().inventory_calls(), 0);
            shared.borrow_mut().queue_membership(Ok(membership()));
            shared
                .borrow_mut()
                .queue_inventory(Ok(inventory(&membership(), NOW, 1)));
            let ProfileSyncWithObservationsOutcome::Refreshed(pair) = service
                .sync_with_observations(ProfileSyncReason::Forced, TTL, NOW)
                .unwrap()
            else {
                panic!("refresh required")
            };
            assert!(pair.observations().is_none());
            assert_eq!(pair.snapshot().metadata.checked_at, NOW);
            assert_eq!(shared.borrow().membership_calls(), 1);
            assert_eq!(shared.borrow().inventory_calls(), 1);
        }

        #[test]
        fn observations_complete_empty_and_forced_reasons_use_one_scoped_read() {
            use crate::sync::ProfileSyncWithObservationsOutcome;
            for reason in [ProfileSyncReason::Preflight, ProfileSyncReason::PostAction] {
                let mut input = inventory(&membership(), NOW, 1);
                input.items.clear();
                let (gateway, calls) = observed_gateway(input);
                let mut repository = InMemoryInventoryRepository::new();
                repository
                    .replace_current(fresh_seed(&membership()))
                    .unwrap();
                let mut service = ProfileSyncService::new(gateway, repository);
                let ProfileSyncWithObservationsOutcome::Refreshed(pair) =
                    service.sync_with_observations(reason, TTL, NOW).unwrap()
                else {
                    panic!("forced refresh required")
                };
                assert_eq!(pair.observations(), Some([].as_slice()));
                assert_eq!(*calls.borrow(), [reason, reason]);
                let (snapshot, observations) = pair.into_parts();
                assert!(snapshot.items.is_empty());
                assert_eq!(observations, Some(vec![]));
            }
        }

        // Deliberately dishonest publication returns test the release boundary.
        // The backing write has succeeded: rejection does NOT promise rollback.
        struct ReturnedSnapshotRepository {
            backing: InMemoryInventoryRepository,
            alter: fn(&mut InventorySnapshot),
            loads: std::cell::Cell<usize>,
            writes: usize,
            fail: bool,
        }
        impl InventoryRepository for ReturnedSnapshotRepository {
            fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
                self.loads.set(self.loads.get() + 1);
                let mut snapshot = self.backing.load_current()?;
                // A post-write reload would contaminate the association.
                if self.writes > 0
                    && let Some(snapshot) = &mut snapshot
                {
                    snapshot.items.clear();
                }
                Ok(snapshot)
            }
            fn replace_current(
                &mut self,
                input: InventorySnapshotInput,
            ) -> Result<InventorySnapshot, AppError> {
                self.writes += 1;
                if self.fail {
                    return Err(AppError::new(ErrorCategory::LocalStorage));
                }
                let mut snapshot = self.backing.replace_current(input)?;
                (self.alter)(&mut snapshot);
                Ok(snapshot)
            }
        }
        fn returned_repository(alter: fn(&mut InventorySnapshot)) -> ReturnedSnapshotRepository {
            ReturnedSnapshotRepository {
                backing: InMemoryInventoryRepository::new(),
                alter,
                loads: std::cell::Cell::new(0),
                writes: 0,
                fail: false,
            }
        }

        #[test]
        fn observations_require_full_publication_content_and_exact_metadata() {
            let alterations: [fn(&mut InventorySnapshot); 20] = [
                |s| s.items[0].item_hash = ItemHash::new(8),
                |s| s.items[0].quantity += 1,
                |s| s.items[0].equipped = true,
                |s| s.items[0].locked = true,
                |s| s.items[0].power = Some(1),
                |s| {
                    s.items[0].perks.push(destiny_core::PerkSummary {
                        perk_hash: destiny_core::PerkHash::new(60),
                        active: true,
                        visible: true,
                    })
                },
                |s| {
                    s.items[0].stats.push(destiny_core::StatSummary {
                        stat_hash: 61,
                        value: 1,
                    })
                },
                |s| {
                    s.items[0].plugs.push(destiny_core::PlugSummary {
                        plug_hash: ItemHash::new(62),
                        enabled: true,
                    })
                },
                |s| {
                    s.items[0]
                        .available_plugs
                        .push(destiny_core::AvailablePlug {
                            socket_index: 0,
                            plug_hash: ItemHash::new(63),
                        })
                },
                |s| {
                    s.stacks.pop();
                },
                |s| {
                    s.stacks[0] = destiny_core::StackObservation::new(
                        ItemHash::new(50),
                        destiny_core::BucketHash::new(51),
                        1,
                        destiny_core::StackContainer::ProfileCurrencies,
                    )
                    .unwrap();
                },
                |s| {
                    s.profile_currencies_coverage =
                        destiny_core::ProfileCurrenciesCoverage::Unavailable
                },
                |s| s.metadata.definition_version = DefinitionVersion::new("other"),
                |s| s.metadata.membership_id = MembershipId::new("4611686018484639000"),
                |s| s.metadata.membership_type = MembershipType::new(1),
                |s| s.metadata.checked_at = Timestamp::new(NOW.seconds() + 1),
                |s| s.metadata.source = SourceMarker::Manifest,
                |s| s.characters[0].name = Some("changed".into()),
                |s| s.items[0].location = ItemLocation::Postmaster(s.characters[0].id.clone()),
                |s| {
                    s.plug_set_entries.push(destiny_core::domain::PlugSetEntry {
                        scope: destiny_core::domain::PlugSetScope::Profile,
                        plug_hash: ItemHash::new(64),
                        can_insert: true,
                        enabled: true,
                    })
                },
            ];
            let mut input = inventory(&membership(), NOW, 1);
            input.characters.push(destiny_core::Character {
                id: "900000000000000001".into(),
                name: None,
                class: None,
                race: None,
                gender: None,
                light: None,
                date_last_played: None,
            });
            input.profile_currencies_coverage = destiny_core::ProfileCurrenciesCoverage::Available;
            let currency = destiny_core::StackObservation::new(
                ItemHash::new(50),
                destiny_core::BucketHash::new(51),
                0,
                destiny_core::StackContainer::ProfileCurrencies,
            )
            .unwrap();
            input.stacks = vec![currency.clone(), currency];
            // The exact complete control succeeds before each isolated one-field
            // alteration is tested against a fresh repository and acquisition.
            let (gateway, _) = observed_gateway(input.clone());
            let mut control = ProfileSyncService::new(gateway, returned_repository(|_| {}));
            let crate::sync::ProfileSyncWithObservationsOutcome::Refreshed(pair) = control
                .sync_with_observations(ProfileSyncReason::Forced, TTL, NOW)
                .unwrap()
            else {
                panic!("valid control must publish")
            };
            assert_eq!(pair.snapshot().stacks, input.stacks);
            assert_eq!(pair.observations().unwrap().len(), 1);
            for alter in alterations {
                let (gateway, _) = observed_gateway(input.clone());
                let mut service = ProfileSyncService::new(gateway, returned_repository(alter));
                let error = service
                    .sync_with_observations(ProfileSyncReason::Forced, TTL, NOW)
                    .expect_err("no proof for a different publication");
                assert_eq!(error.category(), ErrorCategory::ProviderValidation);
                assert_eq!(service.repository().writes, 1);
                assert_eq!(
                    service.repository().loads.get(),
                    1,
                    "only initial freshness read"
                );
                assert!(
                    service
                        .repository()
                        .backing
                        .load_current()
                        .unwrap()
                        .is_some(),
                    "no rollback claim for malicious successful publication"
                );
            }
        }

        #[test]
        fn observations_use_returned_canonical_snapshot_without_post_write_reload() {
            use crate::sync::ProfileSyncWithObservationsOutcome;
            let mut input = inventory(&membership(), NOW, 1);
            let mut second = input.items[0].clone();
            second.instance_id = "instance-0".into();
            input.items.push(second);
            let (gateway, _) = observed_gateway(input);
            let mut service = ProfileSyncService::new(gateway, returned_repository(|_| {}));
            let ProfileSyncWithObservationsOutcome::Refreshed(pair) = service
                .sync_with_observations(ProfileSyncReason::Forced, TTL, NOW)
                .unwrap()
            else {
                panic!("refresh required")
            };
            assert_eq!(
                pair.snapshot(),
                &service
                    .repository()
                    .backing
                    .load_current()
                    .unwrap()
                    .unwrap()
            );
            assert_eq!(pair.snapshot().items[0].instance_id.as_str(), "instance-0");
            assert_eq!(pair.observations().unwrap().len(), 2);
            assert_eq!(service.repository().loads.get(), 1);
            assert_eq!(service.repository().writes, 1);
        }

        #[test]
        fn observations_publication_error_and_requested_membership_reject_without_proof() {
            let (gateway, _) = observed_gateway(inventory(&membership(), NOW, 1));
            let mut repository = returned_repository(|_| {});
            repository.fail = true;
            let mut service = ProfileSyncService::new(gateway, repository);
            assert_eq!(
                service
                    .sync_with_observations(ProfileSyncReason::Forced, TTL, NOW)
                    .unwrap_err()
                    .category(),
                ErrorCategory::LocalStorage
            );
            assert_eq!(service.repository().writes, 1);
            assert!(
                service
                    .repository()
                    .backing
                    .load_current()
                    .unwrap()
                    .is_none()
            );
            for other in [
                membership_with("4611686018484639000", 3),
                membership_with("4611686018484639825", 1),
            ] {
                let (gateway, _) = observed_gateway(inventory(&other, NOW, 1));
                let mut service = ProfileSyncService::new(gateway, returned_repository(|_| {}));
                assert_eq!(
                    service
                        .sync_with_observations(ProfileSyncReason::Forced, TTL, NOW)
                        .unwrap_err()
                        .category(),
                    ErrorCategory::ProviderValidation
                );
                assert_eq!(service.repository().writes, 0);
            }
        }

        #[test]
        fn observations_reacquired_at_same_revision_then_inventory_change_advances() {
            use crate::sync::ProfileSyncWithObservationsOutcome;
            let (gateway, calls) = observed_gateway(inventory(&membership(), NOW, 1));
            let mut service = ProfileSyncService::new(gateway, InMemoryInventoryRepository::new());
            for (offset, quantity, bucket, revision) in
                [(0, 1, 99, 1), (1, 1, 100, 1), (2, 2, 100, 2)]
            {
                let checked_at = Timestamp::new(NOW.seconds() + offset);
                service.gateway.acquisition = Some(acquisition(
                    inventory(&membership(), checked_at, quantity),
                    bucket,
                ));
                let ProfileSyncWithObservationsOutcome::Refreshed(pair) = service
                    .sync_with_observations(ProfileSyncReason::Preflight, TTL, checked_at)
                    .unwrap()
                else {
                    panic!("refresh required")
                };
                assert_eq!(pair.snapshot().metadata.checked_at, checked_at);
                assert_eq!(pair.snapshot().metadata.revision.value(), revision);
                assert_eq!(
                    pair.observations().unwrap()[0].bucket_hash().value(),
                    bucket
                );
                assert_eq!(
                    pair.snapshot(),
                    &service.repository().load_current().unwrap().unwrap()
                );
            }
            assert_eq!(calls.borrow().len(), 6);
        }

        #[test]
        fn observation_owner_rejection_precedes_write() {
            let shared = gateway();
            shared.borrow_mut().queue_membership(Ok(membership()));
            let mut dangling = inventory(&membership(), NOW, 1);
            dangling.items[0].location = ItemLocation::Postmaster("missing-owner".into());
            shared.borrow_mut().queue_inventory(Ok(dangling));
            let mut service = ProfileSyncService::new(
                SharedGateway(Rc::clone(&shared)),
                returned_repository(|_| {}),
            );
            assert_eq!(
                service
                    .sync_with_observations(ProfileSyncReason::Forced, TTL, NOW)
                    .unwrap_err()
                    .category(),
                ErrorCategory::ProviderValidation
            );
            assert_eq!(service.repository().writes, 0);
            assert_eq!(shared.borrow().inventory_calls(), 1);
        }

        #[test]
        fn ordinary_uses_legacy_scoped_override_without_early_pair_validation() {
            struct LegacyOnly;
            impl ProfileGateway for LegacyOnly {
                fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
                    panic!("scoped override required")
                }
                fn load_inventory(
                    &mut self,
                    _: &DestinyMembership,
                ) -> Result<InventorySnapshotInput, AppError> {
                    panic!("scoped override required")
                }
                fn resolve_membership_scoped(
                    &mut self,
                    reason: ProfileSyncReason,
                ) -> Result<DestinyMembership, AppError> {
                    assert_eq!(reason, ProfileSyncReason::Preflight);
                    Ok(membership())
                }
                fn load_inventory_scoped(
                    &mut self,
                    requested: &DestinyMembership,
                    reason: ProfileSyncReason,
                ) -> Result<InventorySnapshotInput, AppError> {
                    assert_eq!(requested, &membership());
                    assert_eq!(reason, ProfileSyncReason::Preflight);
                    let mut input = inventory(requested, NOW, 1);
                    // Invalid before filtering, valid afterwards. Routing through
                    // the new default constructor would reject this too early.
                    input.items[0].plugs.push(destiny_core::PlugSummary {
                        plug_hash: ItemHash::new(60),
                        enabled: false,
                    });
                    Ok(input)
                }
                fn load_inventory_with_observations_scoped(
                    &mut self,
                    _: &DestinyMembership,
                    _: ProfileSyncReason,
                ) -> Result<destiny_core::ProfileAcquisition, AppError> {
                    panic!("ordinary must not reroute through observation override")
                }
            }
            let mut service =
                ProfileSyncService::new(LegacyOnly, InMemoryInventoryRepository::new());
            // Lookup errors still precede structural rejection on the legacy
            // path; the new default acquisition wrapper would reverse that.
            service.lookup_plug = |_, _| Err(AppError::new(ErrorCategory::LocalStorage));
            assert_eq!(
                service
                    .sync(ProfileSyncReason::Preflight, TTL, NOW)
                    .unwrap_err()
                    .category(),
                ErrorCategory::LocalStorage
            );
            service.lookup_plug = |_, _| Ok(destiny_core::Resolution::Missing);
            assert!(
                service
                    .sync(ProfileSyncReason::Preflight, TTL, NOW)
                    .unwrap()
                    .refreshed()
            );
            assert!(
                service.repository().load_current().unwrap().unwrap().items[0]
                    .plugs
                    .is_empty()
            );
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
                armor: Default::default(),
                non_transferrable: None,
                does_postmaster_pull_have_side_effects: None,
                is_instance_item: None,
                max_stack_size: None,
                stack_unique_label: None,
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
            let mut observed_inputs = Vec::new();
            for cosmetic in [61, 64] {
                let mut candidate = inventory(&resolved, NOW, 1);
                candidate.items[0].sockets = vec![destiny_core::IndexedSocket {
                    socket_index: 0,
                    plug_hash: None,
                    enabled: false,
                    visible: false,
                    reusable_plugs: Some(vec![destiny_core::domain::ReusablePlug {
                        plug_hash: ItemHash::new(61),
                        can_insert: false,
                        enabled: false,
                    }]),
                }];
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
                observed_inputs.push(candidate.clone());
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
                assert_eq!(
                    snapshot.items[0].sockets[0]
                        .reusable_plugs
                        .as_ref()
                        .unwrap()[0]
                        .plug_hash,
                    ItemHash::new(61)
                );
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
            // Available runtime observations survive filtering, and comparison
            // uses the filtered candidate rather than the raw acquisition.
            let (_, repository) = service.into_parts();
            let (mut gateway, _) = observed_gateway(observed_inputs[0].clone());
            gateway.acquisition = None;
            let mut service = ProfileSyncService::new_with_definitions(gateway, repository);
            for (index, input) in observed_inputs.into_iter().enumerate() {
                service.gateway.acquisition = Some(acquisition(input, 99 + index as i64));
                let crate::sync::ProfileSyncWithObservationsOutcome::Refreshed(pair) = service
                    .sync_with_observations(ProfileSyncReason::Forced, TTL, NOW)
                    .unwrap()
                else {
                    panic!("refresh required")
                };
                assert_eq!(pair.snapshot().metadata.revision.value(), 1);
                assert_eq!(pair.observations().unwrap().len(), 1);
                assert_eq!(pair.snapshot().items[0].available_plugs.len(), 3);
                assert_eq!(
                    pair.snapshot(),
                    &service.repository().load_current().unwrap().unwrap()
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
