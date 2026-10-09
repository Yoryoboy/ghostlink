//! Instanced-only Postmaster pull contracts, not action authorization.
//!
//! Canonical identifier syntax does not prove observed provider identity,
//! ownership, or Postmaster location. A future fresh service preflight must
//! establish those facts, safety eligibility and capacity before dispatch;
//! constructing a request from opaque user identifiers alone authorizes nothing.
//! No anonymous stack can be converted into this request.

use crate::{AppError, CharacterId, DestinyMembership, ErrorCategory, ItemHash, ItemInstanceId};

/// Immutable intent to pull one genuine instance, with both its hash and ID.
///
/// Contains no credentials. Quantity is always one, never caller-selected.
/// Fields are private and accessors do not permit mutation:
///
/// ```compile_fail
/// use destiny_core::{InstancedPullRequest, ItemHash};
/// fn alter(request: &mut InstancedPullRequest) {
///     request.item_hash = ItemHash::new(0);
/// }
/// ```
///
/// Construction has no quantity parameter:
///
/// ```compile_fail
/// use destiny_core::{CharacterId, DestinyMembership, InstancedPullRequest, ItemHash, ItemInstanceId};
/// fn with_quantity(membership: DestinyMembership) {
///     InstancedPullRequest::new(membership, ItemInstanceId::new("1"),
///         ItemHash::new(1), CharacterId::new("2"), 2);
/// }
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstancedPullRequest {
    membership: DestinyMembership,
    item: ItemInstanceId,
    item_hash: ItemHash,
    character: CharacterId,
}

impl InstancedPullRequest {
    /// Validates wire representability, not identity, ownership or eligibility.
    ///
    /// Item and character IDs must be canonical positive ASCII decimals within
    /// signed int64. Hash must fit uint32, including zero; membership type must
    /// fit int32 without additional provider-enum restrictions. Membership ID
    /// retains the existing [`DestinyMembership`] constructor contract and is
    /// not a field in the pull body. All errors are value-free validation errors.
    pub fn new(
        membership: DestinyMembership,
        item: ItemInstanceId,
        item_hash: ItemHash,
        character: CharacterId,
    ) -> Result<Self, AppError> {
        if !positive_int64_id(item.as_str())
            || !positive_int64_id(character.as_str())
            || u32::try_from(item_hash.value()).is_err()
            || i32::try_from(membership.membership_type().value()).is_err()
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(Self {
            membership,
            item,
            item_hash,
            character,
        })
    }

    pub const fn membership(&self) -> &DestinyMembership {
        &self.membership
    }

    pub const fn item(&self) -> &ItemInstanceId {
        &self.item
    }

    pub const fn item_hash(&self) -> ItemHash {
        self.item_hash
    }

    pub const fn character(&self) -> &CharacterId {
        &self.character
    }

    /// The fixed signed-int32 wire quantity for one instanced item.
    pub const fn quantity(&self) -> i32 {
        1
    }
}

fn positive_int64_id(value: &str) -> bool {
    // Bound bytes before scanning or parsing, including adversarial length bombs.
    !value.is_empty()
        && value.len() <= 19
        && !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<i64>().is_ok()
}

/// Minimal, sanitized classification of a deterministic received refusal.
/// Numeric provider mapping is adapter-owned; no provider text is retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstancedPullRejection {
    Provider(crate::ProviderRejection),
    /// The provider deterministically rejected the request; nothing applied.
    InvalidRequest,
    /// DestinyNoRoomInDestination (Bungie Platform error 1642).
    DestinationFull,
}

impl InstancedPullRejection {
    pub const fn category(self) -> ErrorCategory {
        match self {
            Self::Provider(reason) => reason.category(),
            Self::InvalidRequest | Self::DestinationFull => ErrorCategory::ProviderValidation,
        }
    }
}

/// Evidence from one attempt, never proof of observed recovery or authorization.
/// No variant permits automatic retry. Acknowledged effects may be persisted
/// locally; this core unit supplies no outcome-to-provider-code mapping.
///
/// Acknowledgement deliberately has no applied, recovered or verified helpers:
///
/// ```compile_fail
/// use destiny_core::InstancedPullOutcome;
/// InstancedPullOutcome::Acknowledged.applied();
/// ```
/// ```compile_fail
/// use destiny_core::InstancedPullOutcome;
/// InstancedPullOutcome::Acknowledged.recovered();
/// ```
/// ```compile_fail
/// use destiny_core::InstancedPullOutcome;
/// InstancedPullOutcome::Acknowledged.verified();
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstancedPullOutcome {
    /// Provider acknowledgement only; not observed application or recovery.
    Acknowledged,
    /// Received deterministic refusal with proven non-application.
    Rejected(InstancedPullRejection),
    /// Received authentication refusal after dispatch, not an applied mutation.
    /// Category remains ProviderAuthentication; pre-dispatch auth failure is Err.
    AuthenticationRefused,
    /// Deterministic received transient refusal with proven non-application ONLY.
    /// Unknown HTTP 5xx or post-dispatch failures are not this variant unless
    /// independent evidence proves nothing applied. This does not permit retry.
    Transient,
    /// Dispatch occurred but its effect is unknown, including unknown HTTP 5xx,
    /// lost responses and post-dispatch failures without non-application proof.
    Uncertain,
}

#[cfg(test)]
mod tests {
    use crate::{
        AppError, CharacterId, DestinyMembership, ErrorCategory, InstancedPullGateway,
        InstancedPullOutcome, InstancedPullRejection, InstancedPullRequest, ItemHash,
        ItemInstanceId, MembershipId, MembershipType,
    };

    fn membership(kind: u32) -> DestinyMembership {
        DestinyMembership::new(MembershipType::new(kind), MembershipId::new("00123456789")).unwrap()
    }

    fn request(
        item: &str,
        character: &str,
        hash: i64,
        kind: u32,
    ) -> Result<InstancedPullRequest, AppError> {
        InstancedPullRequest::new(
            membership(kind),
            ItemInstanceId::new(item),
            ItemHash::new(hash),
            CharacterId::new(character),
        )
    }

    fn assert_invalid(result: Result<InstancedPullRequest, AppError>) {
        let error = result.unwrap_err();
        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
        assert_eq!(error.to_string(), "the provider rejected the request");
        assert_eq!(error, AppError::new(ErrorCategory::ProviderValidation));
    }

    #[test]
    fn instanced_pull_accepts_canonical_id_boundaries_and_preserves_accessors() {
        for id in ["1", "9223372036854775807"] {
            let pull = request(id, id, 42, 3).unwrap();
            assert_eq!(pull.item().as_str(), id);
            assert_eq!(pull.character().as_str(), id);
            assert_eq!(pull.item_hash(), ItemHash::new(42));
            assert_eq!(pull.membership(), &membership(3));
            assert_eq!(pull.quantity(), 1_i32);
            assert_eq!(pull.clone(), pull);
        }
    }

    #[test]
    fn instanced_pull_rejects_noncanonical_ids_in_both_positions() {
        let oversized = "1".repeat(10_000);
        for id in [
            "",
            "0",
            "00",
            "01",
            "-1",
            "+1",
            " 1",
            "1 ",
            "1\n",
            "\t1",
            "1\0",
            "１",
            "١",
            "1.0",
            "1e1",
            "9223372036854775808",
            "9999999999999999999",
            "10000000000000000000",
            oversized.as_str(),
        ] {
            assert_invalid(request(id, "1", 42, 3));
            assert_invalid(request("1", id, 42, 3));
        }
    }

    #[test]
    fn instanced_pull_hash_is_uint32_including_zero() {
        for hash in [0, i64::from(u32::MAX)] {
            assert_eq!(
                request("1", "2", hash, 3).unwrap().item_hash().value(),
                hash
            );
        }
        for hash in [-1, i64::MIN, i64::from(u32::MAX) + 1, i64::MAX] {
            assert_invalid(request("1", "2", hash, 3));
        }
    }

    #[test]
    fn instanced_pull_membership_type_is_int32_without_enum_restrictions() {
        for kind in [0, 1, 3, 254, i32::MAX as u32] {
            let pull = request("1", "2", 42, kind).unwrap();
            assert_eq!(pull.membership().membership_type().value(), kind);
        }
        for kind in [i32::MAX as u32 + 1, u32::MAX] {
            assert_invalid(request("1", "2", 42, kind));
        }
    }

    #[test]
    fn instanced_pull_debug_preserves_opaque_identifier_redaction() {
        let pull = request("987654321", "123456789", 42, 3).unwrap();
        let debug = format!("{pull:?}");
        for id in ["987654321", "123456789", "00123456789"] {
            assert!(!debug.contains(id));
        }
    }

    #[test]
    fn instanced_pull_outcomes_are_distinct_bounded_provider_evidence() {
        let outcomes = [
            InstancedPullOutcome::Acknowledged,
            InstancedPullOutcome::Rejected(InstancedPullRejection::InvalidRequest),
            InstancedPullOutcome::AuthenticationRefused,
            InstancedPullOutcome::Transient,
            InstancedPullOutcome::Uncertain,
        ];
        for (index, outcome) in outcomes.iter().enumerate() {
            for other in &outcomes[index + 1..] {
                assert_ne!(outcome, other);
            }
        }
        assert_eq!(
            InstancedPullRejection::InvalidRequest.category(),
            ErrorCategory::ProviderValidation
        );
    }

    #[test]
    fn instanced_pull_gateway_fake_receives_exact_request_once_without_retry() {
        struct RecordingGateway {
            requests: Vec<InstancedPullRequest>,
        }
        impl InstancedPullGateway for RecordingGateway {
            fn pull(
                &mut self,
                request: &InstancedPullRequest,
            ) -> Result<InstancedPullOutcome, AppError> {
                self.requests.push(request.clone());
                Ok(InstancedPullOutcome::Uncertain)
            }
        }
        let pull = request("1", "2", 42, 3).unwrap();
        let mut gateway = RecordingGateway { requests: vec![] };
        let port: &mut dyn InstancedPullGateway = &mut gateway;
        assert_eq!(port.pull(&pull), Ok(InstancedPullOutcome::Uncertain));
        assert_eq!(gateway.requests, vec![pull]);
        // This tests the core seam, not transport, ownership or provider execution.
    }
}
