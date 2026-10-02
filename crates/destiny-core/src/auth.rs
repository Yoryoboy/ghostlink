//! Provider-neutral authorization values and callback outcomes.

use crate::state::Timestamp;
use std::fmt;

macro_rules! opaque_value {
    ($name:ident, $label:literal) => {
        #[derive(Clone, Eq, PartialEq)]
        pub struct $name(crate::redact::Redacted<Vec<u8>>);
        impl $name {
            pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
                Self(crate::redact::Redacted::new(bytes.into()))
            }
            pub fn as_bytes(&self) -> &[u8] {
                self.0.expose_secret().as_slice()
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                crate::redact::opaque(formatter, $label)
            }
        }
    };
}

opaque_value!(AuthorizationState, "AuthorizationState");
opaque_value!(AuthorizationCode, "AuthorizationCode");

/// Validated confidential-client secret, exposed only to trusted adapters.
#[derive(Clone, Eq, PartialEq)]
pub struct ClientSecret(crate::redact::Redacted<Vec<u8>>);

impl ClientSecret {
    /// Bounded like other stored OAuth material.
    pub const MAX_LENGTH: usize = 4 * 1024;

    /// Requires non-empty printable ASCII without whitespace or controls.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Result<Self, crate::error::AppError> {
        let bytes = bytes.into();
        if bytes.is_empty()
            || bytes.len() > Self::MAX_LENGTH
            || !bytes.iter().all(u8::is_ascii_graphic)
        {
            return Err(crate::error::AppError::new(
                crate::error::ErrorCategory::ConfigurationError,
            ));
        }
        Ok(Self(crate::redact::Redacted::new(bytes)))
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.expose_secret().as_slice()
    }
}

impl fmt::Debug for ClientSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::redact::opaque(formatter, "ClientSecret")
    }
}

/// Opaque refresh material, exposed only to trusted adapters through bytes.
#[derive(Clone, Eq, PartialEq)]
pub struct RefreshToken(crate::redact::Redacted<Vec<u8>>);

impl RefreshToken {
    /// Matches the existing credential store's 4 KiB material bound.
    pub const MAX_LENGTH: usize = 4 * 1024;

    /// Rejects empty or oversized material without including it in the error.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Result<Self, crate::error::AppError> {
        let bytes = bytes.into();
        if bytes.is_empty() || bytes.len() > Self::MAX_LENGTH {
            return Err(crate::error::AppError::new(
                crate::error::ErrorCategory::ProviderValidation,
            ));
        }
        Ok(Self(crate::redact::Redacted::new(bytes)))
    }

    /// Exposes secret bytes to a trusted credential-store/provider adapter.
    pub fn as_bytes(&self) -> &[u8] {
        self.0.expose_secret().as_slice()
    }
}

impl fmt::Debug for RefreshToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::redact::opaque(formatter, "RefreshToken")
    }
}

/// Refresh material paired with its provider-reported expiry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshGrant {
    pub token: RefreshToken,
    pub expires_at: Timestamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessCredentialMetadata {
    pub expires_at: Timestamp,
}

#[derive(Clone, Eq, PartialEq)]
pub struct AccessCredential {
    pub metadata: AccessCredentialMetadata,
    pub material: crate::ports::CredentialMaterial,
    pub refresh_grant: Option<RefreshGrant>,
}

impl AccessCredential {
    /// Builds an access-only credential, preserving public-client behavior.
    pub fn new(
        metadata: AccessCredentialMetadata,
        material: crate::ports::CredentialMaterial,
    ) -> Self {
        Self {
            metadata,
            material,
            refresh_grant: None,
        }
    }

    /// Attaches a provider-issued refresh grant, replacing any previous grant.
    pub fn with_refresh_grant(mut self, grant: RefreshGrant) -> Self {
        self.refresh_grant = Some(grant);
        self
    }

    pub fn refresh_grant(&self) -> Option<&RefreshGrant> {
        self.refresh_grant.as_ref()
    }
}

impl fmt::Debug for AccessCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::redact::opaque(formatter, "AccessCredential")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallbackResult {
    Code(AuthorizationCode),
    Denied,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizationCallback {
    pub state: AuthorizationState,
    pub result: CallbackResult,
}

#[cfg(test)]
mod tests {
    use super::{
        AccessCredential, AccessCredentialMetadata, AuthorizationCode, AuthorizationState,
    };
    use crate::ports::CredentialMaterial;
    use crate::state::Timestamp;

    #[test]
    fn client_secret_is_validated_and_opaque() {
        use crate::{ClientSecret, ErrorCategory};
        for bytes in [
            Vec::new(),
            b"has space".to_vec(),
            b"tab\t".to_vec(),
            b"line\n".to_vec(),
            vec![0x7f],
            vec![0x80],
            vec![b'x'; ClientSecret::MAX_LENGTH + 1],
        ] {
            let error = ClientSecret::from_bytes(bytes).expect_err("invalid secret");
            assert_eq!(error.category(), ErrorCategory::ConfigurationError);
        }
        for bytes in [b"secret:+/%".to_vec(), vec![b'x'; ClientSecret::MAX_LENGTH]] {
            let secret = ClientSecret::from_bytes(bytes.clone()).expect("valid secret");
            assert!(secret.as_bytes() == bytes);
            assert_eq!(format!("{secret:?}"), "ClientSecret");
        }
    }

    #[test]
    fn refresh_token_is_bounded_and_opaque() {
        use crate::{ErrorCategory, RefreshToken};

        for bytes in [Vec::new(), vec![b'x'; RefreshToken::MAX_LENGTH + 1]] {
            let error = RefreshToken::from_bytes(bytes).expect_err("invalid token");
            assert_eq!(error.category(), ErrorCategory::ProviderValidation);
            assert!(!format!("{error:?}").contains("xxx"));
        }
        for bytes in [vec![b'x'], vec![b'x'; RefreshToken::MAX_LENGTH]] {
            let token = RefreshToken::from_bytes(bytes.clone()).expect("bounded token");
            assert!(token.as_bytes() == bytes);
        }
        let token = RefreshToken::from_bytes(b"refresh-sentinel").expect("token");
        assert_eq!(format!("{token:?}"), "RefreshToken");
        assert!(!format!("{token:?}").contains("refresh-sentinel"));
    }

    #[test]
    fn access_credential_refresh_grant_is_optional_and_redacted() {
        use crate::{RefreshGrant, RefreshToken};

        let credential = AccessCredential::new(
            AccessCredentialMetadata {
                expires_at: Timestamp::new(100),
            },
            CredentialMaterial::from_bytes(b"access-sentinel"),
        );
        assert_eq!(credential.refresh_grant(), None);
        let grant = RefreshGrant {
            token: RefreshToken::from_bytes(b"refresh-sentinel").expect("token"),
            expires_at: Timestamp::new(200),
        };
        let credential = credential.with_refresh_grant(grant.clone());
        assert_eq!(credential.refresh_grant(), Some(&grant));
        assert_eq!(format!("{credential:?}"), "AccessCredential");
        let rendered = format!("{credential:?} {grant:?}");
        for secret in ["access-sentinel", "refresh-sentinel"] {
            assert!(!rendered.contains(secret));
        }
    }

    #[test]
    fn static_credential_source_returns_independent_clones_repeatedly() {
        use crate::{CredentialSource, RefreshGrant, RefreshToken, StaticCredentialSource};

        let credential = AccessCredential::new(
            AccessCredentialMetadata {
                expires_at: Timestamp::new(100),
            },
            CredentialMaterial::from_bytes(b"access-sentinel"),
        )
        .with_refresh_grant(RefreshGrant {
            token: RefreshToken::from_bytes(b"refresh-sentinel").expect("token"),
            expires_at: Timestamp::new(200),
        });
        let source = StaticCredentialSource::new(credential.clone());
        let shared: &dyn CredentialSource = &source;
        let mut first = shared.current().expect("credential");
        assert_eq!(first, credential);
        first.metadata.expires_at = Timestamp::new(0);
        for _ in 0..3 {
            assert_eq!(shared.current().expect("credential"), credential);
        }
        let public = AccessCredential::new(credential.metadata, credential.material.clone());
        assert_eq!(
            StaticCredentialSource::new(public.clone()).current(),
            Ok(public)
        );
    }

    #[test]
    fn authorization_values_and_access_credentials_render_only_their_type_name() {
        let state = AuthorizationState::from_bytes(b"state-sentinel");
        let code = AuthorizationCode::from_bytes(b"code-sentinel");
        let credential = AccessCredential {
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(1_724_000_000),
            },
            material: CredentialMaterial::from_bytes(b"material-sentinel".to_vec()),
            refresh_grant: None,
        };

        // Each value-hiding type renders exactly its stable type name and never
        // its bytes; there is no `Display` path that could leak them.
        assert_eq!(format!("{state:?}"), "AuthorizationState");
        assert_eq!(format!("{code:?}"), "AuthorizationCode");
        assert_eq!(format!("{credential:?}"), "AccessCredential");
        assert_eq!(format!("{:?}", credential.material), "CredentialMaterial");

        for sentinel in ["state-sentinel", "code-sentinel", "material-sentinel"] {
            let rendered = format!(
                "{state:?} {code:?} {credential:?} {:?}",
                credential.material
            );
            assert!(
                !rendered.contains(sentinel),
                "redaction leaked protected material"
            );
        }

        // The trusted accessors are the only way back to the bytes.
        assert_eq!(state.as_bytes(), b"state-sentinel");
        let expected_code = b"code-sentinel";
        let expected_material = b"material-sentinel";
        assert!(code.as_bytes() == expected_code);
        assert!(credential.material.expose_secret() == expected_material);
    }
}
