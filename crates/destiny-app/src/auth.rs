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
    Authorized(AccessCredentialMetadata),
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
        }
    }

    pub fn state(&self) -> &AuthState {
        &self.state
    }

    pub fn begin(&mut self) -> Result<AuthorizationOutcome, AppError> {
        let now = self.clock.now()?;
        let expires_at = Timestamp::new(
            now.seconds()
                .checked_add(self.ttl_seconds)
                .ok_or_else(|| AppError::new(ErrorCategory::ClockOverflow))?,
        );
        let state = self.random.generate_state()?;
        self.browser.open(AuthorizationRequest {
            state: state.clone(),
            expires_at,
        })?;
        self.state = AuthState::Authorizing { state, expires_at };
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
        let pending = match std::mem::replace(&mut self.state, AuthState::SignedOut) {
            AuthState::Authorizing { state, expires_at } => Some((state, expires_at)),
            _ => None,
        };
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
            Err(_) => {
                self.state = AuthState::ReauthorizationRequired;
                return Ok(AuthorizationOutcome::ExchangeFailed);
            }
        };
        let metadata = credential.metadata;
        self.store.save(credential.material)?;
        self.state = AuthState::Authorized { access: metadata };
        Ok(AuthorizationOutcome::Authorized(metadata))
    }

    pub fn access_status(&mut self) -> Result<AuthorizationOutcome, AppError> {
        let AuthState::Authorized { access } = self.state else {
            return Ok(AuthorizationOutcome::ReauthorizationRequired);
        };
        if self.clock.now()?.seconds() >= access.expires_at.seconds() {
            self.state = AuthState::ReauthorizationRequired;
            Ok(AuthorizationOutcome::ReauthorizationRequired)
        } else {
            Ok(AuthorizationOutcome::Authorized(access))
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
    // Behavioral coverage uses the feature-gated fakes; this compile check keeps the generic seam public.
    #[test]
    fn service_is_provider_neutral() {
        let _ = std::mem::size_of::<AuthorizationOutcome>();
    }
}
