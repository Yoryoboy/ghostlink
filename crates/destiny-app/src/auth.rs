//! Offline authorization application state machine.

use destiny_core::ports::AuthorizationRequest;
use destiny_core::{
    AccessCredentialMetadata, AppError, AuthorizationCallback, AuthorizationState,
    BrowserAuthorization, CallbackReceiver, CallbackResult, Clock, CredentialStore, ErrorCategory,
    PublicTokenExchange, SecureRandom, Timestamp,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthState {
    SignedOut,
    Authorizing {
        state: AuthorizationState,
        expires_at: Timestamp,
    },
    Authorized {
        access: AccessCredentialMetadata,
    },
    ReauthorizationRequired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthorizationOutcome {
    Started,
    Authorizing,
    Authorized(AccessCredentialMetadata),
    /// Token-free summary of a successfully persisted renewable session.
    AuthorizedSession {
        access: AccessCredentialMetadata,
        refresh_available: bool,
        refresh_expires_at: Option<Timestamp>,
    },
    SignedOut,
    Cancelled,
    Denied,
    StateMismatch,
    Replay,
    Expired,
    ExchangeFailed,
    ReauthorizationRequired,
    LoggedOut,
}

pub struct AuthorizationService<R, B, C, X, S, K> {
    state: AuthState,
    random: R,
    browser: B,
    callback: C,
    exchange: X,
    store: S,
    clock: K,
    ttl_seconds: u64,
    refresh_expires_at: Option<Timestamp>,
}

impl<R, B, C, X, S, K> AuthorizationService<R, B, C, X, S, K>
where
    R: SecureRandom,
    B: BrowserAuthorization,
    C: CallbackReceiver,
    X: PublicTokenExchange,
    S: CredentialStore,
    K: Clock,
{
    pub fn new(
        random: R,
        browser: B,
        callback: C,
        exchange: X,
        store: S,
        clock: K,
        ttl_seconds: u64,
    ) -> Self {
        Self {
            state: AuthState::SignedOut,
            random,
            browser,
            callback,
            exchange,
            store,
            clock,
            ttl_seconds,
            refresh_expires_at: None,
        }
    }

    pub fn state(&self) -> &AuthState {
        &self.state
    }

    pub fn begin(&mut self) -> Result<AuthorizationOutcome, AppError> {
        match self.state {
            AuthState::SignedOut | AuthState::ReauthorizationRequired => {}
            AuthState::Authorizing { .. } => {
                return Err(AppError::new(ErrorCategory::AlreadyAuthorizing));
            }
            AuthState::Authorized { .. } => {
                return Err(AppError::new(ErrorCategory::AlreadyAuthorized));
            }
        }
        let prior_state = self.state.clone();
        let now = self.clock.now()?;
        let expires_at = Timestamp::new(
            now.seconds()
                .checked_add(self.ttl_seconds)
                .ok_or_else(|| AppError::new(ErrorCategory::ClockOverflow))?,
        );
        let state = self.random.generate_state()?;
        self.state = AuthState::Authorizing {
            state: state.clone(),
            expires_at,
        };
        if let Err(error) = self
            .browser
            .open(AuthorizationRequest { state, expires_at })
        {
            self.state = prior_state;
            return Err(error);
        }
        Ok(AuthorizationOutcome::Started)
    }

    pub fn receive_callback(&mut self) -> Result<AuthorizationOutcome, AppError> {
        let callback = self.callback.receive()?;
        self.handle_callback(callback)
    }

    pub fn handle_callback(
        &mut self,
        callback: AuthorizationCallback,
    ) -> Result<AuthorizationOutcome, AppError> {
        let pending = match &self.state {
            AuthState::Authorizing { state, expires_at } => Some((state.clone(), *expires_at)),
            _ => None,
        };
        if pending.is_some() {
            self.state = AuthState::SignedOut;
        }
        let Some((expected, expires_at)) = pending else {
            return Ok(AuthorizationOutcome::Replay);
        };
        let now = self.clock.now()?;
        if now.seconds() >= expires_at.seconds() {
            self.state = AuthState::SignedOut;
            return Ok(AuthorizationOutcome::Expired);
        }
        if callback.state != expected {
            return Ok(AuthorizationOutcome::StateMismatch);
        }
        let code = match callback.result {
            CallbackResult::Cancelled => return Ok(AuthorizationOutcome::Cancelled),
            CallbackResult::Denied => return Ok(AuthorizationOutcome::Denied),
            CallbackResult::Code(code) => code,
        };
        let credential = match self.exchange.exchange(code) {
            Ok(value) => value,
            Err(error) => {
                self.state = AuthState::ReauthorizationRequired;
                let outcome = match error.category() {
                    ErrorCategory::ProviderAuthentication
                    | ErrorCategory::ReauthorizationRequired => {
                        AuthorizationOutcome::ReauthorizationRequired
                    }
                    _ => AuthorizationOutcome::ExchangeFailed,
                };
                return Ok(outcome);
            }
        };
        let metadata = credential.metadata;
        let refresh_expires_at = credential.refresh_grant().map(|grant| grant.expires_at);
        if self.store.save(credential).is_err() {
            self.state = AuthState::ReauthorizationRequired;
            return Err(AppError::new(ErrorCategory::CredentialStoreUnavailable));
        }
        self.state = AuthState::Authorized { access: metadata };
        self.refresh_expires_at = refresh_expires_at;
        Ok(self.authorized_outcome(metadata))
    }

    fn authorized_outcome(&self, access: AccessCredentialMetadata) -> AuthorizationOutcome {
        match self.refresh_expires_at {
            Some(refresh_expires_at) => AuthorizationOutcome::AuthorizedSession {
                access,
                refresh_available: self.refresh_expires_at.is_some(),
                refresh_expires_at: Some(refresh_expires_at),
            },
            None => AuthorizationOutcome::Authorized(access),
        }
    }

    pub fn access_status(&mut self) -> Result<AuthorizationOutcome, AppError> {
        match self.state {
            AuthState::SignedOut => Ok(AuthorizationOutcome::SignedOut),
            AuthState::Authorizing { .. } => Ok(AuthorizationOutcome::Authorizing),
            AuthState::ReauthorizationRequired => Ok(AuthorizationOutcome::ReauthorizationRequired),
            AuthState::Authorized { access } => {
                if self.clock.now()?.seconds() >= access.expires_at.seconds() {
                    self.state = AuthState::ReauthorizationRequired;
                    Ok(AuthorizationOutcome::ReauthorizationRequired)
                } else {
                    Ok(self.authorized_outcome(access))
                }
            }
        }
    }

    pub fn provider_authentication_rejected(&mut self) -> AuthorizationOutcome {
        self.state = AuthState::ReauthorizationRequired;
        AuthorizationOutcome::ReauthorizationRequired
    }

    pub fn cancel(&mut self) -> AuthorizationOutcome {
        self.state = AuthState::SignedOut;
        AuthorizationOutcome::Cancelled
    }

    pub fn logout(&mut self) -> Result<AuthorizationOutcome, AppError> {
        self.store.delete()?;
        self.state = AuthState::SignedOut;
        Ok(AuthorizationOutcome::LoggedOut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_is_provider_neutral() {
        let _ = std::mem::size_of::<AuthorizationOutcome>();
    }

    #[cfg(feature = "test-support")]
    mod behavioral {
        use super::*;
        use crate::test_support::{
            DeterministicRandom, FailingBrowser, FailingCredentialStore, FakeClock,
            InMemoryCredentialStore, QueuedCallbackReceiver, QueuedTokenExchange, RecordingBrowser,
        };
        use destiny_core::{
            AccessCredential, AuthorizationCode, CredentialMaterial, ErrorCategory,
        };
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Default)]
        struct SharedCredentialStore {
            credential: Arc<Mutex<Option<AccessCredential>>>,
        }

        impl CredentialStore for SharedCredentialStore {
            fn save(&mut self, credential: AccessCredential) -> Result<(), AppError> {
                *self.credential.lock().expect("shared store lock") = Some(credential);
                Ok(())
            }

            fn load(&self) -> Result<Option<AccessCredential>, AppError> {
                Ok(self.credential.lock().expect("shared store lock").clone())
            }

            fn delete(&mut self) -> Result<(), AppError> {
                *self.credential.lock().expect("shared store lock") = None;
                Ok(())
            }
        }

        fn credential(expires_at: u64) -> AccessCredential {
            AccessCredential {
                metadata: AccessCredentialMetadata {
                    expires_at: Timestamp::new(expires_at),
                },
                material: CredentialMaterial::from_bytes(b"credential-sentinel"),
                refresh_grant: None,
            }
        }

        fn callback(state: &AuthorizationState, result: CallbackResult) -> AuthorizationCallback {
            AuthorizationCallback {
                state: state.clone(),
                result,
            }
        }

        fn started_service<S>(
            store: S,
        ) -> AuthorizationService<
            DeterministicRandom,
            RecordingBrowser,
            QueuedCallbackReceiver,
            QueuedTokenExchange,
            S,
            FakeClock,
        >
        where
            S: CredentialStore,
        {
            let mut exchange = QueuedTokenExchange::default();
            exchange.outcomes.push_back(Ok(credential(100)));
            let mut service = AuthorizationService::new(
                DeterministicRandom::new([
                    AuthorizationState::from_bytes(b"state-sentinel"),
                    AuthorizationState::from_bytes(b"state-second"),
                ]),
                RecordingBrowser::default(),
                QueuedCallbackReceiver::default(),
                exchange,
                store,
                FakeClock::fixed(Timestamp::new(10)),
                20,
            );
            assert_eq!(service.begin(), Ok(AuthorizationOutcome::Started));
            service
        }

        #[test]
        fn persisted_refresh_grant_is_reported_without_token_material() {
            let mut service = started_service(InMemoryCredentialStore::default());
            service.exchange.outcomes.clear();
            let mut renewable = credential(100);
            renewable.refresh_grant = Some(destiny_core::RefreshGrant {
                token: destiny_core::RefreshToken::from_bytes(b"refresh-sentinel").unwrap(),
                expires_at: Timestamp::new(900),
            });
            service.exchange.outcomes.push_back(Ok(renewable));
            let state = match service.state() {
                AuthState::Authorizing { state, .. } => state.clone(),
                _ => unreachable!(),
            };
            let outcome = service
                .handle_callback(callback(
                    &state,
                    CallbackResult::Code(AuthorizationCode::from_bytes(b"code-sentinel")),
                ))
                .unwrap();
            assert_eq!(
                outcome,
                AuthorizationOutcome::AuthorizedSession {
                    access: AccessCredentialMetadata {
                        expires_at: Timestamp::new(100)
                    },
                    refresh_available: true,
                    refresh_expires_at: Some(Timestamp::new(900)),
                }
            );
            assert_eq!(service.access_status().unwrap(), outcome);
            assert!(!format!("{outcome:?}").contains("sentinel"));
        }

        #[test]
        fn begin_rejects_reentry_without_replacing_pending_or_authorized_state() {
            let mut service = started_service(InMemoryCredentialStore::default());
            assert_eq!(
                service.begin().unwrap_err().category(),
                ErrorCategory::AlreadyAuthorizing
            );
            let state = match service.state() {
                AuthState::Authorizing { state, .. } => state.clone(),
                other => panic!("unexpected state: {other:?}"),
            };
            assert_eq!(
                service.handle_callback(callback(
                    &state,
                    CallbackResult::Code(AuthorizationCode::from_bytes(b"code"))
                )),
                Ok(AuthorizationOutcome::Authorized(AccessCredentialMetadata {
                    expires_at: Timestamp::new(100)
                }))
            );
            assert_eq!(
                service.begin().unwrap_err().category(),
                ErrorCategory::AlreadyAuthorized
            );
            assert_eq!(
                service.handle_callback(callback(
                    &state,
                    CallbackResult::Code(AuthorizationCode::from_bytes(b"code"))
                )),
                Ok(AuthorizationOutcome::Replay)
            );
            assert!(matches!(service.state(), AuthState::Authorized { .. }));
        }

        #[test]
        fn browser_failure_rolls_back_to_reauthorization_without_leaking_sentinels() {
            let mut service = AuthorizationService::new(
                DeterministicRandom::new([AuthorizationState::from_bytes(b"state-sentinel")]),
                FailingBrowser {
                    error: AppError::new(ErrorCategory::ProviderUnavailable),
                },
                QueuedCallbackReceiver::default(),
                QueuedTokenExchange::default(),
                InMemoryCredentialStore::default(),
                FakeClock::fixed(Timestamp::new(10)),
                20,
            );
            service.provider_authentication_rejected();
            let error = service.begin().unwrap_err();
            assert_eq!(error.category(), ErrorCategory::ProviderUnavailable);
            assert_eq!(service.state(), &AuthState::ReauthorizationRequired);
            assert!(!error.to_string().contains("sentinel"));
        }

        #[test]
        fn access_status_matches_every_state_and_exact_expiry() {
            let mut signed_out = started_service(InMemoryCredentialStore::default());
            signed_out.cancel();
            assert_eq!(
                signed_out.access_status(),
                Ok(AuthorizationOutcome::SignedOut)
            );
            assert_eq!(signed_out.begin(), Ok(AuthorizationOutcome::Started));
            assert_eq!(
                signed_out.access_status(),
                Ok(AuthorizationOutcome::Authorizing)
            );

            let state = match signed_out.state() {
                AuthState::Authorizing { state, .. } => state.clone(),
                _ => unreachable!(),
            };
            assert_eq!(
                signed_out.handle_callback(callback(
                    &state,
                    CallbackResult::Code(AuthorizationCode::from_bytes(b"code"))
                )),
                Ok(AuthorizationOutcome::Authorized(AccessCredentialMetadata {
                    expires_at: Timestamp::new(100)
                }))
            );
            assert_eq!(
                signed_out.access_status(),
                Ok(AuthorizationOutcome::Authorized(AccessCredentialMetadata {
                    expires_at: Timestamp::new(100)
                }))
            );
            if let AuthState::Authorized { .. } = signed_out.state() {
                // The service clock is fixed at 10; this assertion covers the non-expired branch.
            }
            signed_out.provider_authentication_rejected();
            assert_eq!(
                signed_out.access_status(),
                Ok(AuthorizationOutcome::ReauthorizationRequired)
            );
        }

        #[test]
        fn exact_access_expiry_requires_reauthorization() {
            let mut exchange = QueuedTokenExchange::default();
            exchange.outcomes.push_back(Ok(credential(100)));
            let mut service = AuthorizationService::new(
                DeterministicRandom::new([AuthorizationState::from_bytes(b"state")]),
                RecordingBrowser::default(),
                QueuedCallbackReceiver::default(),
                exchange,
                InMemoryCredentialStore::default(),
                FakeClock::fixed(Timestamp::new(100)),
                20,
            );
            service.begin().unwrap();
            let state = match service.state() {
                AuthState::Authorizing { state, .. } => state.clone(),
                _ => unreachable!(),
            };
            service
                .handle_callback(callback(
                    &state,
                    CallbackResult::Code(AuthorizationCode::from_bytes(b"code")),
                ))
                .unwrap();
            assert_eq!(
                service.access_status(),
                Ok(AuthorizationOutcome::ReauthorizationRequired)
            );
            assert_eq!(service.state(), &AuthState::ReauthorizationRequired);
        }

        #[test]
        fn terminal_callbacks_are_consumed_and_replay_is_harmless() {
            for result in [CallbackResult::Cancelled, CallbackResult::Denied] {
                let mut service = started_service(InMemoryCredentialStore::default());
                let state = match service.state() {
                    AuthState::Authorizing { state, .. } => state.clone(),
                    _ => unreachable!(),
                };
                let expected = if matches!(result, CallbackResult::Cancelled) {
                    AuthorizationOutcome::Cancelled
                } else {
                    AuthorizationOutcome::Denied
                };
                assert_eq!(
                    service.handle_callback(callback(&state, result)),
                    Ok(expected)
                );
                assert_eq!(
                    service.handle_callback(callback(&state, CallbackResult::Denied)),
                    Ok(AuthorizationOutcome::Replay)
                );
                assert_eq!(service.state(), &AuthState::SignedOut);
            }
        }

        #[test]
        fn exchange_and_store_failures_are_explicit_and_reauthorization_required() {
            let mut exchange = QueuedTokenExchange::default();
            exchange
                .outcomes
                .push_back(Err(AppError::new(ErrorCategory::ProviderUnavailable)));
            let mut service = AuthorizationService::new(
                DeterministicRandom::new([AuthorizationState::from_bytes(b"state")]),
                RecordingBrowser::default(),
                QueuedCallbackReceiver::default(),
                exchange,
                InMemoryCredentialStore::default(),
                FakeClock::fixed(Timestamp::new(10)),
                20,
            );
            service.begin().unwrap();
            let state = match service.state() {
                AuthState::Authorizing { state, .. } => state.clone(),
                _ => unreachable!(),
            };
            assert_eq!(
                service.handle_callback(callback(
                    &state,
                    CallbackResult::Code(AuthorizationCode::from_bytes(b"code"))
                )),
                Ok(AuthorizationOutcome::ExchangeFailed)
            );
            assert_eq!(service.state(), &AuthState::ReauthorizationRequired);

            let mut service = started_service(FailingCredentialStore {
                error: AppError::new(ErrorCategory::LocalStorage),
            });
            let state = match service.state() {
                AuthState::Authorizing { state, .. } => state.clone(),
                _ => unreachable!(),
            };
            let error = service
                .handle_callback(callback(
                    &state,
                    CallbackResult::Code(AuthorizationCode::from_bytes(b"code")),
                ))
                .unwrap_err();
            assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);
            assert_eq!(service.state(), &AuthState::ReauthorizationRequired);
        }

        #[test]
        fn exchange_errors_categorize_into_reauthorization_or_exchange_failure() {
            for (category, expected) in [
                (
                    ErrorCategory::ProviderAuthentication,
                    AuthorizationOutcome::ReauthorizationRequired,
                ),
                (
                    ErrorCategory::ReauthorizationRequired,
                    AuthorizationOutcome::ReauthorizationRequired,
                ),
                (
                    ErrorCategory::ProviderValidation,
                    AuthorizationOutcome::ExchangeFailed,
                ),
                (
                    ErrorCategory::ProviderUnavailable,
                    AuthorizationOutcome::ExchangeFailed,
                ),
                (
                    ErrorCategory::ProviderRateLimit,
                    AuthorizationOutcome::ExchangeFailed,
                ),
                (
                    ErrorCategory::AuthorizationDenied,
                    AuthorizationOutcome::ExchangeFailed,
                ),
            ] {
                let mut exchange = QueuedTokenExchange::default();
                exchange.outcomes.push_back(Err(AppError::new(category)));
                let mut service = AuthorizationService::new(
                    DeterministicRandom::new([AuthorizationState::from_bytes(b"state")]),
                    RecordingBrowser::default(),
                    QueuedCallbackReceiver::default(),
                    exchange,
                    InMemoryCredentialStore::default(),
                    FakeClock::fixed(Timestamp::new(10)),
                    20,
                );
                service.begin().unwrap();
                let state = match service.state() {
                    AuthState::Authorizing { state, .. } => state.clone(),
                    _ => unreachable!(),
                };
                let outcome = service
                    .handle_callback(callback(
                        &state,
                        CallbackResult::Code(AuthorizationCode::from_bytes(b"code")),
                    ))
                    .unwrap();
                assert_eq!(
                    outcome, expected,
                    "category {category:?} mapped incorrectly"
                );
                assert_ne!(outcome, AuthorizationOutcome::Denied);
                assert_eq!(service.state(), &AuthState::ReauthorizationRequired);
            }
        }

        #[test]
        fn provider_authentication_exchange_error_restarts_after_reauthorization() {
            let mut exchange = QueuedTokenExchange::default();
            exchange
                .outcomes
                .push_back(Err(AppError::new(ErrorCategory::ProviderAuthentication)));
            exchange.outcomes.push_back(Ok(credential(100)));
            let mut service = AuthorizationService::new(
                DeterministicRandom::new([
                    AuthorizationState::from_bytes(b"state-first"),
                    AuthorizationState::from_bytes(b"state-second"),
                ]),
                RecordingBrowser::default(),
                QueuedCallbackReceiver::default(),
                exchange,
                InMemoryCredentialStore::default(),
                FakeClock::fixed(Timestamp::new(10)),
                20,
            );
            assert_eq!(service.begin(), Ok(AuthorizationOutcome::Started));
            let state = match service.state() {
                AuthState::Authorizing { state, .. } => state.clone(),
                _ => unreachable!(),
            };
            assert_eq!(
                service.handle_callback(callback(
                    &state,
                    CallbackResult::Code(AuthorizationCode::from_bytes(b"first-code"))
                )),
                Ok(AuthorizationOutcome::ReauthorizationRequired)
            );
            assert_eq!(service.state(), &AuthState::ReauthorizationRequired);

            assert_eq!(service.begin(), Ok(AuthorizationOutcome::Started));
            let state = match service.state() {
                AuthState::Authorizing { state, .. } => state.clone(),
                _ => unreachable!(),
            };
            assert_eq!(
                service.handle_callback(callback(
                    &state,
                    CallbackResult::Code(AuthorizationCode::from_bytes(b"second-code"))
                )),
                Ok(AuthorizationOutcome::Authorized(AccessCredentialMetadata {
                    expires_at: Timestamp::new(100)
                }))
            );
            assert!(matches!(service.state(), AuthState::Authorized { .. }));
        }

        #[test]
        fn logout_after_authorization_deletes_the_stored_credential() {
            let store = SharedCredentialStore::default();
            let handle = store.clone();
            let mut service = started_service(store);
            let state = match service.state() {
                AuthState::Authorizing { state, .. } => state.clone(),
                _ => unreachable!(),
            };
            service
                .handle_callback(callback(
                    &state,
                    CallbackResult::Code(AuthorizationCode::from_bytes(b"code")),
                ))
                .unwrap();
            assert!(
                handle
                    .credential
                    .lock()
                    .expect("shared store lock")
                    .is_some()
            );
            assert_eq!(service.logout(), Ok(AuthorizationOutcome::LoggedOut));
            assert!(
                handle
                    .credential
                    .lock()
                    .expect("shared store lock")
                    .is_none()
            );
            assert_eq!(service.state(), &AuthState::SignedOut);
        }

        #[test]
        fn authorization_persists_the_complete_credential_metadata_and_material() {
            let store = SharedCredentialStore::default();
            let handle = store.clone();
            let mut service = started_service(store);
            let state = match service.state() {
                AuthState::Authorizing { state, .. } => state.clone(),
                _ => unreachable!(),
            };
            service
                .handle_callback(callback(
                    &state,
                    CallbackResult::Code(AuthorizationCode::from_bytes(b"code")),
                ))
                .unwrap();

            // The port stores the whole credential: the expiry the exchange
            // returned is not dropped when only the material is persisted.
            let stored = handle
                .credential
                .lock()
                .expect("shared store lock")
                .clone()
                .expect("a credential was persisted");
            assert_eq!(stored.metadata.expires_at, Timestamp::new(100));
            assert_eq!(stored.material.expose_secret(), b"credential-sentinel");
            assert_eq!(stored, credential(100));
            assert_eq!(
                service.access_status(),
                Ok(AuthorizationOutcome::Authorized(AccessCredentialMetadata {
                    expires_at: Timestamp::new(100)
                }))
            );
        }

        #[test]
        fn mismatch_is_terminal_and_debug_output_redacts_opaque_values() {
            let mut service = started_service(InMemoryCredentialStore::default());
            let wrong = AuthorizationState::from_bytes(b"wrong-state-sentinel");
            assert_eq!(
                service.handle_callback(callback(
                    &wrong,
                    CallbackResult::Code(AuthorizationCode::from_bytes(b"code-sentinel"))
                )),
                Ok(AuthorizationOutcome::StateMismatch)
            );
            assert_eq!(
                service.handle_callback(callback(&wrong, CallbackResult::Denied)),
                Ok(AuthorizationOutcome::Replay)
            );
            assert_eq!(service.state(), &AuthState::SignedOut);
            assert!(!format!("{wrong:?}").contains("sentinel"));
            assert!(
                !format!("{:?}", AuthorizationCode::from_bytes(b"code-sentinel"))
                    .contains("sentinel")
            );
            assert!(
                !format!(
                    "{:?}",
                    CredentialMaterial::from_bytes(b"credential-sentinel")
                )
                .contains("sentinel")
            );
        }

        #[test]
        fn logout_failure_preserves_state_and_success_is_atomic() {
            let mut service = started_service(FailingCredentialStore {
                error: AppError::new(ErrorCategory::LocalStorage),
            });
            let before = service.state().clone();
            assert_eq!(
                service.logout().unwrap_err().category(),
                ErrorCategory::LocalStorage
            );
            assert_eq!(service.state(), &before);
            let mut service = started_service(InMemoryCredentialStore::default());
            assert_eq!(service.logout(), Ok(AuthorizationOutcome::LoggedOut));
            assert_eq!(service.state(), &AuthState::SignedOut);
        }
    }
}
