//! Serialized session refresh policy, independent of provider and storage adapters.

use destiny_core::{
    AccessCredential, AppError, Clock, CredentialSource, CredentialStore, ErrorCategory,
    TokenRefresh,
};
use std::sync::{Arc, Mutex};

/// Runtime mode selection with one load before any token transport is built.
/// Clones of the renewable variant share the same refresh lock and cache.
pub enum ConfiguredCredentialSource<S: CredentialStore, R: TokenRefresh, C: Clock> {
    Static(destiny_core::StaticCredentialSource),
    Refreshing(RefreshingCredentialSource<S, R, C>),
}

impl<S: CredentialStore, R: TokenRefresh, C: Clock> Clone for ConfiguredCredentialSource<S, R, C> {
    fn clone(&self) -> Self {
        match self {
            Self::Static(source) => Self::Static(source.clone()),
            Self::Refreshing(source) => Self::Refreshing(source.clone()),
        }
    }
}

impl<S: CredentialStore, R: TokenRefresh, C: Clock> ConfiguredCredentialSource<S, R, C> {
    pub fn new<F>(store: S, refresh: Option<F>, clock: C) -> Result<Self, AppError>
    where
        F: FnOnce() -> Result<R, AppError>,
    {
        let credential = store
            .load()?
            .ok_or_else(|| AppError::new(ErrorCategory::AuthorizationRequired))?;
        match refresh {
            None => Ok(Self::Static(destiny_core::StaticCredentialSource::new(
                credential,
            ))),
            Some(build) => Ok(Self::Refreshing(RefreshingCredentialSource {
                state: Arc::new(Mutex::new(SessionState {
                    store,
                    refresh: build()?,
                    credential: Some(credential),
                })),
                clock: Arc::new(clock),
            })),
        }
    }
}

impl<S: CredentialStore, R: TokenRefresh, C: Clock> CredentialSource
    for ConfiguredCredentialSource<S, R, C>
{
    fn current(&self) -> Result<AccessCredential, AppError> {
        match self {
            Self::Static(source) => source.current(),
            Self::Refreshing(source) => source.current(),
        }
    }
}

/// Refresh at or inside this many seconds before access expiry.
pub const ACCESS_REFRESH_MARGIN_SECONDS: u64 = 60;

/// Loads the store once at construction, avoiding repeated keyring reads on every
/// provider request. The lock covers refresh, persistence, and cache replacement
/// so concurrent callers see the same newly refreshed credential. Clones share
/// that state, allowing profile and transfer gateways to use one session.
///
/// Persistence is best-effort after refresh: a failed save must not discard a
/// usable (possibly rotated) token. This process retains it; the next process
/// may need reauthorization. No stored credential is deleted on refresh failure.
pub struct RefreshingCredentialSource<S: CredentialStore, R: TokenRefresh, C: Clock> {
    state: Arc<Mutex<SessionState<S, R>>>,
    clock: Arc<C>,
}

struct SessionState<S, R> {
    store: S,
    refresh: R,
    credential: Option<AccessCredential>,
}

impl<S: CredentialStore, R: TokenRefresh, C: Clock> Clone for RefreshingCredentialSource<S, R, C> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            clock: Arc::clone(&self.clock),
        }
    }
}

impl<S: CredentialStore, R: TokenRefresh, C: Clock> RefreshingCredentialSource<S, R, C> {
    pub fn new(store: S, refresh: R, clock: C) -> Result<Self, AppError> {
        let credential = store.load()?;
        Ok(Self {
            state: Arc::new(Mutex::new(SessionState {
                store,
                refresh,
                credential,
            })),
            clock: Arc::new(clock),
        })
    }
}

impl<S: CredentialStore, R: TokenRefresh, C: Clock> CredentialSource
    for RefreshingCredentialSource<S, R, C>
{
    fn current(&self) -> Result<AccessCredential, AppError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AppError::new(ErrorCategory::CredentialStoreUnavailable))?;
        let credential = state
            .credential
            .clone()
            .ok_or_else(|| AppError::new(ErrorCategory::AuthorizationRequired))?;
        let now = self.clock.now()?.seconds();
        // Saturation preserves the strict margin comparison even near u64::MAX.
        if now.saturating_add(ACCESS_REFRESH_MARGIN_SECONDS)
            < credential.metadata.expires_at.seconds()
        {
            return Ok(credential);
        }
        let Some(grant) = credential
            .refresh_grant()
            .filter(|grant| now < grant.expires_at.seconds())
        else {
            // Without a usable grant, an access token inside the margin is
            // still valid until it actually expires.
            if now < credential.metadata.expires_at.seconds() {
                return Ok(credential);
            }
            return Err(AppError::new(ErrorCategory::ReauthorizationRequired));
        };
        let mut refreshed = state.refresh.refresh(&grant.token)?;
        if refreshed.refresh_grant().is_none() {
            refreshed = refreshed.with_refresh_grant(grant.clone());
        }
        let _ = state.store.save(refreshed.clone());
        state.credential = Some(refreshed.clone());
        Ok(refreshed)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use crate::test_support::{FakeClock, InMemoryCredentialStore};
    use destiny_core::{
        AccessCredentialMetadata, CredentialMaterial, RefreshGrant, RefreshToken, Timestamp,
    };
    use std::sync::{Arc, Barrier};

    struct ScriptedRefresh {
        result: Result<AccessCredential, AppError>,
        calls: usize,
    }
    impl TokenRefresh for ScriptedRefresh {
        fn refresh(&mut self, token: &RefreshToken) -> Result<AccessCredential, AppError> {
            assert_eq!(token.as_bytes(), b"old-refresh");
            self.calls += 1;
            self.result.clone()
        }
    }
    fn credential(expiry: u64, grant: Option<(&str, u64)>) -> AccessCredential {
        AccessCredential {
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(expiry),
            },
            material: CredentialMaterial::from_bytes(b"access-sentinel"),
            refresh_grant: grant.map(|(token, expiry)| RefreshGrant {
                token: RefreshToken::from_bytes(token.as_bytes()).unwrap(),
                expires_at: Timestamp::new(expiry),
            }),
        }
    }
    fn source(
        old: Option<AccessCredential>,
        result: Result<AccessCredential, AppError>,
    ) -> RefreshingCredentialSource<InMemoryCredentialStore, ScriptedRefresh, FakeClock> {
        let mut store = InMemoryCredentialStore::default();
        if let Some(old) = old {
            store.save(old).unwrap();
        }
        RefreshingCredentialSource::new(
            store,
            ScriptedRefresh { result, calls: 0 },
            FakeClock::fixed(Timestamp::new(100)),
        )
        .unwrap()
    }
    #[test]
    fn configured_session_loads_before_refresh_factory_and_shares_refresh() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Counting(Arc<AtomicUsize>);
        impl TokenRefresh for Counting {
            fn refresh(&mut self, _: &RefreshToken) -> Result<AccessCredential, AppError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(credential(1000, None))
            }
        }
        let count = Arc::new(AtomicUsize::new(0));
        for renewable in [false, true] {
            let mut store = InMemoryCredentialStore::default();
            store
                .save(credential(150, Some(("old-refresh", 900))))
                .unwrap();
            let source = super::ConfiguredCredentialSource::new(
                store,
                renewable.then_some(|| Ok(Counting(count.clone()))),
                FakeClock::fixed(Timestamp::new(100)),
            )
            .unwrap();
            let expected_expiry = if renewable { 1000 } else { 150 };
            for shared in [source.clone(), source] {
                assert_eq!(
                    shared.current().unwrap().metadata.expires_at.seconds(),
                    expected_expiry
                );
            }
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let result = super::ConfiguredCredentialSource::new(
            InMemoryCredentialStore::default(),
            Some(|| -> Result<Counting, AppError> {
                panic!("missing credential must precede transport construction")
            }),
            FakeClock::fixed(Timestamp::new(100)),
        );
        assert_eq!(
            result.err().unwrap().category(),
            ErrorCategory::AuthorizationRequired
        );
    }

    #[test]
    fn store_load_failure_preserves_its_sanitized_category() {
        let result = RefreshingCredentialSource::new(
            crate::test_support::FailingCredentialStore {
                error: AppError::new(ErrorCategory::LocalStorage),
            },
            ScriptedRefresh {
                result: Ok(credential(1000, None)),
                calls: 0,
            },
            FakeClock::fixed(Timestamp::new(100)),
        );
        assert_eq!(
            result.err().unwrap().category(),
            ErrorCategory::LocalStorage
        );
    }

    #[test]
    fn margin_near_timestamp_limit_does_not_wrap_into_freshness() {
        let mut store = InMemoryCredentialStore::default();
        store.save(credential(u64::MAX, None)).unwrap();
        let source = RefreshingCredentialSource::new(
            store,
            ScriptedRefresh {
                result: Ok(credential(1000, None)),
                calls: 0,
            },
            FakeClock::fixed(Timestamp::new(u64::MAX - 1)),
        )
        .unwrap();
        // Saturated margin falls through to the grant path; with no grant the
        // still-unexpired credential is returned without a refresh attempt.
        assert!(source.current().is_ok());
        assert_eq!(source.state.lock().unwrap().refresh.calls, 0);
    }

    #[test]
    fn fresh_credential_never_refreshes() {
        let old = credential(161, None);
        let source = source(
            Some(old.clone()),
            Err(AppError::new(ErrorCategory::ProviderUnavailable)),
        );
        assert_eq!(source.current().unwrap(), old);
        assert_eq!(source.state.lock().unwrap().refresh.calls, 0);
    }
    #[test]
    fn margin_refresh_is_saved_and_omitted_grant_is_carried_over() {
        for expiry in [99, 100, 159, 160] {
            let old = credential(expiry, Some(("old-refresh", 1000)));
            let expected = credential(1000, Some(("old-refresh", 1000)));
            let source = source(Some(old), Ok(credential(1000, None)));
            assert_eq!(source.current().unwrap(), expected);
            assert_eq!(source.current().unwrap(), expected);
            let state = source.state.lock().unwrap();
            assert_eq!(state.refresh.calls, 1);
            assert_eq!(state.store.load().unwrap(), Some(expected));
        }
    }
    #[test]
    fn rotation_retains_the_new_grant() {
        let new = credential(1000, Some(("new-refresh", 2000)));
        let source = source(
            Some(credential(160, Some(("old-refresh", 1000)))),
            Ok(new.clone()),
        );
        assert_eq!(source.current().unwrap(), new);
        assert_eq!(
            source.state.lock().unwrap().store.load().unwrap(),
            Some(new)
        );
    }
    #[test]
    fn missing_or_expired_grant_requires_reauthorization_without_refresh() {
        for grant in [None, Some(("old-refresh", 99)), Some(("old-refresh", 100))] {
            let old = credential(100, grant);
            let source = source(Some(old.clone()), Ok(credential(1000, None)));
            let error = source.current().unwrap_err();
            assert_eq!(error.category(), ErrorCategory::ReauthorizationRequired);
            assert!(!format!("{error:?} {error}").contains("sentinel"));
            let state = source.state.lock().unwrap();
            assert_eq!(state.refresh.calls, 0);
            assert_eq!(state.store.load().unwrap(), Some(old));
        }
    }
    #[test]
    fn unexpired_credential_without_usable_grant_is_returned_inside_margin() {
        for grant in [None, Some(("old-refresh", 99)), Some(("old-refresh", 100))] {
            let old = credential(130, grant);
            let source = source(Some(old.clone()), Ok(credential(1000, None)));
            assert_eq!(source.current().unwrap(), old);
            assert_eq!(source.state.lock().unwrap().refresh.calls, 0);
        }
    }

    #[test]
    fn refresh_rejection_preserves_store_and_cache() {
        let old = credential(160, Some(("old-refresh", 1000)));
        let source = source(
            Some(old.clone()),
            Err(AppError::new(ErrorCategory::ReauthorizationRequired)),
        );
        assert_eq!(
            source.current().unwrap_err().category(),
            ErrorCategory::ReauthorizationRequired
        );
        let state = source.state.lock().unwrap();
        assert_eq!(state.store.load().unwrap(), Some(old.clone()));
        assert_eq!(state.credential, Some(old));
    }
    #[test]
    fn missing_credential_requires_authorization() {
        let source = source(None, Ok(credential(1000, None)));
        assert_eq!(
            source.current().unwrap_err().category(),
            ErrorCategory::AuthorizationRequired
        );
        assert_eq!(source.state.lock().unwrap().refresh.calls, 0);
    }
    #[test]
    fn concurrent_callers_refresh_once() {
        let source = source(
            Some(credential(160, Some(("old-refresh", 1000)))),
            Ok(credential(1000, None)),
        );
        let barrier = Arc::new(Barrier::new(3));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let source = source.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    source.current().unwrap()
                })
            })
            .collect();
        barrier.wait();
        for thread in threads {
            assert_eq!(
                thread.join().unwrap().metadata.expires_at,
                Timestamp::new(1000)
            );
        }
        assert_eq!(source.state.lock().unwrap().refresh.calls, 1);
    }
    struct SaveFailureStore(InMemoryCredentialStore);
    impl CredentialStore for SaveFailureStore {
        fn load(&self) -> Result<Option<AccessCredential>, AppError> {
            self.0.load()
        }
        fn save(&mut self, _: AccessCredential) -> Result<(), AppError> {
            Err(AppError::new(ErrorCategory::LocalStorage))
        }
        fn delete(&mut self) -> Result<(), AppError> {
            panic!("refresh never deletes")
        }
    }
    #[test]
    fn save_failure_keeps_refreshed_credential_usable_in_process() {
        let old = credential(160, Some(("old-refresh", 1000)));
        let mut store = InMemoryCredentialStore::default();
        store.save(old.clone()).unwrap();
        let source = RefreshingCredentialSource::new(
            SaveFailureStore(store),
            ScriptedRefresh {
                result: Ok(credential(1000, None)),
                calls: 0,
            },
            FakeClock::fixed(Timestamp::new(100)),
        )
        .unwrap();
        let expected = credential(1000, Some(("old-refresh", 1000)));
        assert_eq!(source.current().unwrap(), expected);
        assert_eq!(source.current().unwrap(), expected);
        let state = source.state.lock().unwrap();
        assert_eq!(state.refresh.calls, 1);
        assert_eq!(state.store.load().unwrap(), Some(old));
    }
}
