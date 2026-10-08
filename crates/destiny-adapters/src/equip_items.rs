//! Credential-safe official EquipItems adapter. Requires MoveEquipDestinyItems.
//! Caller paces at 0.1s; items not on the character may be ignored by Bungie.
//! One dispatch only, with no retry or refresh.
use crate::transfer::{HttpTransferTransport, ensure_unexpired};
use destiny_core::{
    AppError, Clock, CredentialSource, EquipItemResult, EquipItemStatus, EquipItemsGateway,
    EquipItemsOutcome, EquipItemsRequest, ErrorCategory, StaticCredentialSource,
};
use serde_json::{Value, json};
use std::{collections::HashMap, fmt};

pub use crate::transfer::{
    TransferApiKey as EquipItemsApiKey, TransferBearerToken as EquipItemsBearerToken,
    TransferHttpRequest as EquipItemsHttpRequest, TransferHttpResponse as EquipItemsHttpResponse,
    TransferTransportError as EquipItemsTransportError,
};

pub const EQUIP_ITEMS_ENDPOINT: &str =
    "https://www.bungie.net/Platform/Destiny2/Actions/Items/EquipItems/";
pub const EQUIP_ITEMS_RESPONSE_LIMIT: usize = crate::transfer::TRANSFER_RESPONSE_LIMIT;

pub fn is_allowed_equip_items_url(url: &str) -> bool {
    url == EQUIP_ITEMS_ENDPOINT
}
pub trait EquipItemsTransport {
    fn post(
        &mut self,
        request: &EquipItemsHttpRequest<'_>,
    ) -> Result<EquipItemsHttpResponse, EquipItemsTransportError>;
}

/// Shares hardened transfer machinery, not any other action's allowlist.
pub struct HttpEquipItemsTransport(HttpTransferTransport);
impl HttpEquipItemsTransport {
    pub fn new() -> Result<Self, AppError> {
        HttpTransferTransport::new().map(Self)
    }
}
impl fmt::Debug for HttpEquipItemsTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpEquipItemsTransport")
            .finish_non_exhaustive()
    }
}
impl EquipItemsTransport for HttpEquipItemsTransport {
    fn post(
        &mut self,
        request: &EquipItemsHttpRequest<'_>,
    ) -> Result<EquipItemsHttpResponse, EquipItemsTransportError> {
        if !is_allowed_equip_items_url(request.url()) {
            return Err(EquipItemsTransportError::Configuration);
        }
        self.0.post_allowlisted(request)
    }
}

pub struct BungieEquipItemsGateway<T, C, S = StaticCredentialSource> {
    transport: T,
    clock: C,
    source: S,
    api_key: EquipItemsApiKey,
}
impl<T, C, S> BungieEquipItemsGateway<T, C, S> {
    pub fn new(
        transport: T,
        clock: C,
        source: S,
        api_key: EquipItemsApiKey,
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
impl<T, C, S> fmt::Debug for BungieEquipItemsGateway<T, C, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BungieEquipItemsGateway")
            .finish_non_exhaustive()
    }
}
impl<T: EquipItemsTransport, C: Clock, S: CredentialSource> EquipItemsGateway
    for BungieEquipItemsGateway<T, C, S>
{
    fn equip_items(&mut self, request: &EquipItemsRequest) -> Result<EquipItemsOutcome, AppError> {
        let credential = self.source.current()?;
        let bearer = std::str::from_utf8(credential.material.expose_secret())
            .map_err(|_| AppError::new(ErrorCategory::ConfigurationError))
            .and_then(|token| EquipItemsBearerToken::new(token.to_owned()))?;
        let membership = i32::try_from(request.membership().membership_type().value())
            .map_err(|_| AppError::new(ErrorCategory::ProviderValidation))?;
        let ids: Vec<_> = request.items().iter().map(|id| id.as_str()).collect();
        let body = json!({"itemIds": ids, "characterId": request.character().as_str(), "membershipType": membership}).to_string();
        ensure_unexpired(self.clock.now()?, credential.metadata.expires_at)?;
        let http = EquipItemsHttpRequest::new(
            EQUIP_ITEMS_ENDPOINT,
            body,
            Some(&self.api_key),
            Some(&bearer),
        );
        let mut response = match self.transport.post(&http) {
            Ok(response) => response,
            Err(EquipItemsTransportError::NotDispatched) => {
                return Err(AppError::new(ErrorCategory::ProviderUnavailable));
            }
            Err(EquipItemsTransportError::Configuration) => {
                return Err(AppError::new(ErrorCategory::ConfigurationError));
            }
            Err(EquipItemsTransportError::Indeterminate) => {
                return Ok(EquipItemsOutcome::Uncertain);
            }
        };
        Ok(classify_response(&mut response, request))
    }
}

fn classify_response(
    response: &mut EquipItemsHttpResponse,
    request: &EquipItemsRequest,
) -> EquipItemsOutcome {
    use EquipItemsOutcome as O;
    match response.status() {
        200 => {}
        401 | 403 => return O::AuthenticationRefused,
        429 => return O::Transient,
        400..=499 => return O::Refused,
        _ => return O::Uncertain,
    }
    let Some(body) = response.json_body(response.content_length()) else {
        return O::Uncertain;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return O::Uncertain;
    };
    let Some(code) = value.get("ErrorCode").and_then(Value::as_i64) else {
        return O::Uncertain;
    };
    if i32::try_from(code).is_err() {
        return O::Uncertain;
    }
    if code != 1 {
        return match value.get("ErrorStatus").and_then(Value::as_str) {
            Some("WebAuthRequired" | "AccessTokenHasExpired" | "AccessTokenInvalid") => {
                O::AuthenticationRefused
            }
            Some("DestinyThrottledByGameServer" | "ThrottleLimitExceeded") => O::Transient,
            Some(
                "DestinyCharacterNotInTower"
                | "DestinyCannotPerformActionAtThisLocation"
                | "DestinyCanOnlyEquipInGame"
                | "DestinyAccountMustBeOffline"
                | "DestinyCharacterNotFound"
                | "DestinyItemNotFound"
                | "DestinyItemUniqueEquipRestricted"
                | "DestinyItemUnequippable"
                | "DestinyItemFailedLevelCheck"
                | "DestinyItemFailedUnlockCheck",
            ) => O::Refused,
            _ => O::Uncertain,
        };
    }
    decode_results(&value, request).unwrap_or(O::Uncertain)
}

fn decode_results(value: &Value, request: &EquipItemsRequest) -> Option<EquipItemsOutcome> {
    let results = value.get("Response")?.get("equipResults")?.as_array()?;
    if results.len() > request.items().len() {
        return None;
    }
    let mut statuses = HashMap::new();
    for result in results {
        // Never convert int64 identifiers through floating point.
        let id = match result.get("itemInstanceId")? {
            Value::String(id) if id.parse::<i64>().is_ok() => id.clone(),
            Value::Number(id) => id.as_i64()?.to_string(),
            _ => return None,
        };
        if !request.items().iter().any(|item| item.as_str() == id) {
            return None;
        }
        let code = i32::try_from(result.get("equipStatus")?.as_i64()?).ok()?;
        let status = if code == 1 {
            EquipItemStatus::Success
        } else {
            EquipItemStatus::Failure(code)
        };
        if statuses.insert(id, status).is_some() {
            return None;
        }
    }
    let results = request
        .items()
        .iter()
        .map(|item| EquipItemResult {
            item: item.clone(),
            status: statuses
                .get(item.as_str())
                .copied()
                .unwrap_or(EquipItemStatus::NotReported),
        })
        .collect();
    Some(EquipItemsOutcome::Acknowledged(results))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_rejects_before_credentials() {
        let mut transport = HttpEquipItemsTransport::new().unwrap();
        let key = EquipItemsApiKey::new("key-canary").unwrap();
        let bearer = EquipItemsBearerToken::new("token-canary").unwrap();
        for url in [
            crate::equip::EQUIP_ENDPOINT.to_owned(),
            crate::loadout_action::LOADOUT_ACTION_ENDPOINT.to_owned(),
            crate::transfer::TRANSFER_ENDPOINT.to_owned(),
            crate::lock::LOCK_ENDPOINT.to_owned(),
            format!("{EQUIP_ITEMS_ENDPOINT}?x=1"),
            format!("{EQUIP_ITEMS_ENDPOINT}#x"),
            EQUIP_ITEMS_ENDPOINT.trim_end_matches('/').to_owned(),
            EQUIP_ITEMS_ENDPOINT.replace("https:", "http:"),
            EQUIP_ITEMS_ENDPOINT.replace("www.bungie.net", "evil.example"),
            EQUIP_ITEMS_ENDPOINT.replace("EquipItems", "equipitems"),
            EQUIP_ITEMS_ENDPOINT.replace("Items/", "Items/../Items/"),
        ] {
            crate::transfer::CREDENTIAL_ACCESSES.with(|count| count.set(0));
            assert_eq!(
                transport
                    .post(&EquipItemsHttpRequest::new(
                        &url,
                        "{}".into(),
                        Some(&key),
                        Some(&bearer)
                    ))
                    .unwrap_err(),
                EquipItemsTransportError::Configuration
            );
            crate::transfer::CREDENTIAL_ACCESSES.with(|count| assert_eq!(count.get(), 0));
        }
    }
}
