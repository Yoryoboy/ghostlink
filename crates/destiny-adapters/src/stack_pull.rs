//! One-shot anonymous stack pulls. OAuth MoveEquipDestinyItems is required.
//! The documented 0.1 s action throttle is caller-owned; never retry here.
//! Reuses the exact instanced-pull allowlist and hardened bounded transport.

pub use crate::instanced_pull::{
    HttpInstancedPullTransport as HttpStackPullTransport,
    INSTANCED_PULL_ENDPOINT as STACK_PULL_ENDPOINT,
    INSTANCED_PULL_RESPONSE_LIMIT as STACK_PULL_RESPONSE_LIMIT,
    InstancedPullApiKey as StackPullApiKey, InstancedPullBearerToken as StackPullBearerToken,
    InstancedPullHttpRequest as StackPullHttpRequest,
    InstancedPullHttpResponse as StackPullHttpResponse,
    InstancedPullTransport as StackPullTransport,
    InstancedPullTransportError as StackPullTransportError,
    is_allowed_instanced_pull_url as is_allowed_stack_pull_url,
};
use crate::transfer::ensure_unexpired;
use destiny_core::{
    AppError, Clock, CredentialSource, ErrorCategory, StackPullGateway, StackPullOutcome,
    StackPullRequest, StaticCredentialSource,
};
use serde::{Deserialize, Deserializer};
use std::{collections::BTreeMap, fmt};

pub struct BungieStackPullGateway<T, C, S = StaticCredentialSource> {
    transport: T,
    clock: C,
    source: S,
    api_key: StackPullApiKey,
}
impl<T, C, S> BungieStackPullGateway<T, C, S> {
    pub fn new(
        transport: T,
        clock: C,
        source: S,
        api_key: StackPullApiKey,
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
impl<T, C, S> fmt::Debug for BungieStackPullGateway<T, C, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BungieStackPullGateway")
            .finish_non_exhaustive()
    }
}
impl<T: StackPullTransport, C: Clock, S: CredentialSource> StackPullGateway
    for BungieStackPullGateway<T, C, S>
{
    fn pull(&mut self, request: &StackPullRequest) -> Result<StackPullOutcome, AppError> {
        let credential = self.source.current()?;
        let bearer = std::str::from_utf8(credential.material.expose_secret())
            .map_err(|_| AppError::new(ErrorCategory::ConfigurationError))
            .and_then(|token| StackPullBearerToken::new(token.to_owned()))?;
        ensure_unexpired(self.clock.now()?, credential.metadata.expires_at)?;
        let http = StackPullHttpRequest::new(
            STACK_PULL_ENDPOINT,
            build_body(request)?,
            Some(&self.api_key),
            Some(&bearer),
        );
        let mut response = match self.transport.post(&http) {
            Ok(response) => response,
            Err(StackPullTransportError::Throttled(seconds)) => {
                return Err(AppError::bungie_rate_limit(seconds));
            }
            Err(StackPullTransportError::NotDispatched) => {
                return Err(AppError::new(ErrorCategory::ProviderUnavailable));
            }
            Err(StackPullTransportError::Configuration) => {
                return Err(AppError::new(ErrorCategory::ConfigurationError));
            }
            Err(StackPullTransportError::Indeterminate) => return Ok(StackPullOutcome::Uncertain),
        };
        Ok(classify_response(&mut response))
    }
}
fn build_body(request: &StackPullRequest) -> Result<String, AppError> {
    serde_json::to_string(&serde_json::json!({
        "itemReferenceHash": request.item_hash().value(), "stackSize": request.quantity(),
        "itemId": request.item_id(), "characterId": request.character().as_str(),
        "transferToVault": false,
        "membershipType": request.membership().membership_type().value(),
    }))
    .map_err(|_| AppError::new(ErrorCategory::ConfigurationError))
}
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Envelope {
    response: i32,
    error_code: i32,
    throttle_seconds: i32,
    #[serde(default, deserialize_with = "present")]
    error_status: Option<String>,
    #[serde(default, deserialize_with = "present")]
    message: Option<String>,
    #[serde(default, deserialize_with = "present")]
    message_data: Option<BTreeMap<String, String>>,
    #[serde(default, deserialize_with = "present")]
    detailed_error_trace: Option<String>,
}
fn pull_rejection(reason: destiny_core::ProviderRejection) -> StackPullOutcome {
    StackPullOutcome::Rejected(
        if reason == destiny_core::ProviderRejection::DestinationFull {
            destiny_core::StackPullRejection::DestinationFull
        } else {
            destiny_core::StackPullRejection::Provider(reason)
        },
    )
}

fn classify_response(response: &mut StackPullHttpResponse) -> StackPullOutcome {
    if let Some(reason) = response.status_error_rejection() {
        return pull_rejection(reason);
    }
    match response.status() {
        200 => {}
        401 | 403 => return StackPullOutcome::AuthenticationRefused,
        _ => return StackPullOutcome::Uncertain,
    }
    let Some(body) = response.json_body(response.content_length()) else {
        return StackPullOutcome::Uncertain;
    };
    let Ok(envelope) = serde_json::from_slice::<Envelope>(&body) else {
        return StackPullOutcome::Uncertain;
    };
    // These fields are validated, not surfaced or interpreted as function-result
    // semantics. In particular Response need not be zero to acknowledge.
    let _ = (
        envelope.response,
        envelope.message,
        envelope.message_data,
        envelope.detailed_error_trace,
    );
    if let Some(reason) = u64::try_from(envelope.error_code)
        .ok()
        .and_then(crate::bungie_errors::rejection)
    {
        return pull_rejection(reason);
    }
    if envelope.error_code == 1
        && envelope.throttle_seconds >= 0
        && envelope
            .error_status
            .as_deref()
            .is_none_or(|s| s == "Success")
    {
        StackPullOutcome::Acknowledged
    } else {
        // Unknown non-success codes remain uncertain, never guessed refusals.
        StackPullOutcome::Uncertain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::{CharacterId, DestinyMembership, ItemHash, MembershipId, MembershipType};
    #[test]
    fn numeric_action_rejections() {
        // Bungie also reports game refusals with HTTP 500; the envelope decides.
        for status in [200, 500, 503] {
            crate::bungie_errors::assert_action_codes(|value| {
                let body = value.to_string().into_bytes();
                let mut response = StackPullHttpResponse::new(
                    status,
                    Some(body.len() as u64),
                    Some("application/json".into()),
                    Box::new(std::io::Cursor::new(body)),
                );
                match classify_response(&mut response) {
                    StackPullOutcome::Rejected(destiny_core::StackPullRejection::Provider(
                        reason,
                    )) => Some(reason),
                    StackPullOutcome::Rejected(
                        destiny_core::StackPullRejection::DestinationFull,
                    ) => Some(destiny_core::ProviderRejection::DestinationFull),
                    StackPullOutcome::Uncertain => None,
                    other => panic!("unexpected {other:?}"),
                }
            });
        }
    }
    #[test]
    fn status_keeps_its_meaning_without_a_recognized_refusal() {
        use crate::bungie_errors::StatusProbe;
        crate::bungie_errors::assert_status_precedence(
            StatusProbe::Uncertain,
            |status, media, value| {
                let bytes = value.to_string().into_bytes();
                let mut response = StackPullHttpResponse::new(
                    status,
                    Some(bytes.len() as u64),
                    Some(media.into()),
                    Box::new(std::io::Cursor::new(bytes)),
                );
                match classify_response(&mut response) {
                    StackPullOutcome::Rejected(destiny_core::StackPullRejection::Provider(
                        reason,
                    )) => StatusProbe::Refused(reason),
                    StackPullOutcome::AuthenticationRefused => StatusProbe::AuthenticationRefused,
                    StackPullOutcome::Transient => StatusProbe::Transient,
                    StackPullOutcome::Uncertain => StatusProbe::Uncertain,
                    other => panic!("unexpected {other:?}"),
                }
            },
        );
    }
    #[test]
    fn numeric_no_room_refuses_and_unknown_codes_stay_uncertain() {
        for code in [1642, 1643, 999] {
            for status in ["DestinyNoRoomInDestination", "Success", "untrusted"] {
                let body = serde_json::json!({"Response":0,"ErrorCode":code,"ThrottleSeconds":0,"ErrorStatus":status}).to_string();
                let mut response = StackPullHttpResponse::new(
                    200,
                    Some(body.len() as u64),
                    Some("application/json".into()),
                    Box::new(std::io::Cursor::new(body.into_bytes())),
                );
                assert_eq!(
                    classify_response(&mut response),
                    if code == 1642 {
                        StackPullOutcome::Rejected(
                            destiny_core::StackPullRejection::DestinationFull,
                        )
                    } else {
                        StackPullOutcome::Uncertain
                    }
                );
            }
        }
    }
    #[test]
    fn stack_pull_exact_body() {
        for amount in [1, 17, i32::MAX as i64] {
            let request = StackPullRequest::new(
                DestinyMembership::new(MembershipType::new(3), MembershipId::new("123")).unwrap(),
                ItemHash::new(u32::MAX as i64),
                CharacterId::new("456"),
                amount,
            )
            .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&build_body(&request).unwrap()).unwrap(),
                serde_json::json!({"itemReferenceHash":u32::MAX, "stackSize":amount,
                    "transferToVault":false, "itemId":"0", "characterId":"456", "membershipType":3})
            );
        }
    }
}
