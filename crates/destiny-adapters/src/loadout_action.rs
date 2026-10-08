//! Credential-safe, single-call official EquipLoadout adapter.
//!
//! Requires MoveEquipDestinyItems; callers must pace at one action/second.
//! Bungie allows orbit, social spaces or offline characters. No retry or refresh.
use crate::transfer::{HttpTransferTransport, ensure_unexpired};
use destiny_core::{
    AppError, Clock, CredentialSource, EquipLoadoutRequest, ErrorCategory, LoadoutActionGateway,
    LoadoutActionOutcome, LoadoutActionRejection, StaticCredentialSource,
};
use serde_json::{Value, json};
use std::fmt;

pub use crate::transfer::{
    TransferApiKey as LoadoutActionApiKey, TransferBearerToken as LoadoutActionBearerToken,
    TransferHttpRequest as LoadoutActionHttpRequest,
    TransferHttpResponse as LoadoutActionHttpResponse,
    TransferTransportError as LoadoutActionTransportError,
};

pub const LOADOUT_ACTION_ENDPOINT: &str =
    "https://www.bungie.net/Platform/Destiny2/Actions/Loadouts/EquipLoadout/";
pub const LOADOUT_ACTION_RESPONSE_LIMIT: usize = crate::transfer::TRANSFER_RESPONSE_LIMIT;

pub fn is_allowed_loadout_action_url(url: &str) -> bool {
    url == LOADOUT_ACTION_ENDPOINT
}

pub trait LoadoutActionTransport {
    fn post(
        &mut self,
        request: &LoadoutActionHttpRequest<'_>,
    ) -> Result<LoadoutActionHttpResponse, LoadoutActionTransportError>;
}

/// Reuses the lock adapter's hardened session machinery, not its allowlist.
pub struct HttpLoadoutActionTransport(HttpTransferTransport);
impl HttpLoadoutActionTransport {
    pub fn new() -> Result<Self, AppError> {
        HttpTransferTransport::new().map(Self)
    }
}
impl fmt::Debug for HttpLoadoutActionTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpLoadoutActionTransport")
            .finish_non_exhaustive()
    }
}
impl LoadoutActionTransport for HttpLoadoutActionTransport {
    fn post(
        &mut self,
        request: &LoadoutActionHttpRequest<'_>,
    ) -> Result<LoadoutActionHttpResponse, LoadoutActionTransportError> {
        if !is_allowed_loadout_action_url(request.url()) {
            return Err(LoadoutActionTransportError::Configuration);
        }
        self.0.post_allowlisted(request)
    }
}

pub struct BungieLoadoutActionGateway<T, C, S = StaticCredentialSource> {
    transport: T,
    clock: C,
    source: S,
    api_key: LoadoutActionApiKey,
}
impl<T, C, S> BungieLoadoutActionGateway<T, C, S> {
    pub fn new(
        transport: T,
        clock: C,
        source: S,
        api_key: LoadoutActionApiKey,
    ) -> Result<Self, AppError> {
        Ok(Self {
            transport,
            clock,
            source,
            api_key,
        })
    }
    pub fn transport(&self) -> &T {
        &self.transport
    }
    pub fn into_transport(self) -> T {
        self.transport
    }
}
impl<T, C, S> fmt::Debug for BungieLoadoutActionGateway<T, C, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BungieLoadoutActionGateway")
            .finish_non_exhaustive()
    }
}
impl<T: LoadoutActionTransport, C: Clock, S: CredentialSource> LoadoutActionGateway
    for BungieLoadoutActionGateway<T, C, S>
{
    fn equip_loadout(
        &mut self,
        request: &EquipLoadoutRequest,
    ) -> Result<LoadoutActionOutcome, AppError> {
        let credential = self.source.current()?;
        let bearer = std::str::from_utf8(credential.material.expose_secret())
            .map_err(|_| AppError::new(ErrorCategory::ConfigurationError))
            .and_then(|token| LoadoutActionBearerToken::new(token.to_owned()))?;
        let membership = i32::try_from(request.membership().membership_type().value())
            .map_err(|_| AppError::new(ErrorCategory::ProviderValidation))?;
        let body = json!({"loadoutIndex": request.loadout_index(), "characterId": request.character().as_str(), "membershipType": membership}).to_string();
        ensure_unexpired(self.clock.now()?, credential.metadata.expires_at)?;
        let http = LoadoutActionHttpRequest::new(
            LOADOUT_ACTION_ENDPOINT,
            body,
            Some(&self.api_key),
            Some(&bearer),
        );
        let mut response = match self.transport.post(&http) {
            Ok(response) => response,
            Err(LoadoutActionTransportError::NotDispatched) => {
                return Err(AppError::new(ErrorCategory::ProviderUnavailable));
            }
            Err(LoadoutActionTransportError::Configuration) => {
                return Err(AppError::new(ErrorCategory::ConfigurationError));
            }
            Err(LoadoutActionTransportError::Indeterminate) => {
                return Ok(LoadoutActionOutcome::Uncertain);
            }
        };
        classify_response(&mut response)
    }
}

fn classify_response(
    response: &mut LoadoutActionHttpResponse,
) -> Result<LoadoutActionOutcome, AppError> {
    use LoadoutActionOutcome as Outcome;
    match response.status() {
        200 => {}
        401 | 403 => return Ok(Outcome::AuthenticationRefused),
        429 => return Ok(Outcome::Transient),
        500..=599 => return Ok(Outcome::Uncertain),
        400..=499 => return Ok(Outcome::Rejected(LoadoutActionRejection::InvalidRequest)),
        _ => return Ok(Outcome::Uncertain),
    }
    let Some(body) = response.json_body(response.content_length()) else {
        return Ok(Outcome::Uncertain);
    };
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return Ok(Outcome::Uncertain);
    };
    match value.get("ErrorCode").and_then(Value::as_u64) {
        Some(1) => Ok(Outcome::Applied),
        Some(_) => Ok(classify_error_status(
            value.get("ErrorStatus").and_then(Value::as_str),
        )),
        None => Ok(Outcome::Uncertain),
    }
}

fn classify_error_status(status: Option<&str>) -> LoadoutActionOutcome {
    use LoadoutActionOutcome as Outcome;
    let rejection = match status {
        Some(
            "DestinyCharacterNotInTower"
            | "DestinyCannotPerformActionAtThisLocation"
            | "DestinyCanOnlyEquipInGame"
            | "DestinyAccountMustBeOffline",
        ) => LoadoutActionRejection::RestrictedLocation,
        Some("DestinyCharacterNotFound") => LoadoutActionRejection::CharacterNotFound,
        Some("DestinyThrottledByGameServer" | "ThrottleLimitExceeded") => {
            return Outcome::Transient;
        }
        _ => return Outcome::Uncertain,
    };
    Outcome::Rejected(rejection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_rejects_before_reading_credentials() {
        let mut transport = HttpLoadoutActionTransport::new().unwrap();
        let key = LoadoutActionApiKey::new("key-canary").unwrap();
        let bearer = LoadoutActionBearerToken::new("token-canary").unwrap();
        for url in [
            crate::equip::EQUIP_ENDPOINT,
            crate::lock::LOCK_ENDPOINT,
            crate::transfer::TRANSFER_ENDPOINT,
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/EquipItems/",
            "https://www.bungie.net/Platform/Destiny2/Actions/Loadouts/EquipLoadout/?x=1",
        ] {
            crate::transfer::CREDENTIAL_ACCESSES.with(|count| count.set(0));
            assert_eq!(
                transport
                    .post(&LoadoutActionHttpRequest::new(
                        url,
                        "{}".into(),
                        Some(&key),
                        Some(&bearer)
                    ))
                    .unwrap_err(),
                LoadoutActionTransportError::Configuration
            );
            crate::transfer::CREDENTIAL_ACCESSES.with(|count| assert_eq!(count.get(), 0));
        }
    }
}
