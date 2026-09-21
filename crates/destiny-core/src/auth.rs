//! Provider-neutral authorization values and callback outcomes.

use crate::state::Timestamp;
use std::fmt;

macro_rules! opaque_value {
    ($name:ident, $label:literal) => {
        #[derive(Clone, Eq, PartialEq)]
        pub struct $name(Vec<u8>);
        impl $name {
            pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
                Self(bytes.into())
            }
            pub fn as_bytes(&self) -> &[u8] {
                &self.0
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str($label)
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
        formatter.write_str("AccessCredential")
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
