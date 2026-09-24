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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessCredentialMetadata {
    pub expires_at: Timestamp,
}

#[derive(Clone, Eq, PartialEq)]
pub struct AccessCredential {
    pub metadata: AccessCredentialMetadata,
    pub material: crate::ports::CredentialMaterial,
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
    fn authorization_values_and_access_credentials_render_only_their_type_name() {
        let state = AuthorizationState::from_bytes(b"state-sentinel");
        let code = AuthorizationCode::from_bytes(b"code-sentinel");
        let credential = AccessCredential {
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(1_724_000_000),
            },
            material: CredentialMaterial::from_bytes(b"material-sentinel".to_vec()),
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
            assert!(!rendered.contains(sentinel), "redaction leaked {sentinel}");
        }

        // The trusted accessors are the only way back to the bytes.
        assert_eq!(state.as_bytes(), b"state-sentinel");
        assert_eq!(code.as_bytes(), b"code-sentinel");
        assert_eq!(credential.material.expose_secret(), b"material-sentinel");
    }
}
