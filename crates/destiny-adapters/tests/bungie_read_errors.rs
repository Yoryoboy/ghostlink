use destiny_adapters::{decode_effective_membership, decode_profile};
use destiny_core::{
    DefinitionVersion, DestinyMembership, ErrorCategory, MembershipId, MembershipType, Timestamp,
};

#[test]
fn body_codes_are_typed_on_reads() {
    let membership =
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("123")).unwrap();
    for (code, category) in [
        (35, ErrorCategory::ProviderRateLimit),
        (36, ErrorCategory::ProviderRateLimit),
        (37, ErrorCategory::ProviderRateLimit),
        (54, ErrorCategory::ProviderRateLimit),
        (55, ErrorCategory::ProviderRateLimit),
        (56, ErrorCategory::ProviderRateLimit),
        (57, ErrorCategory::ProviderRateLimit),
        (1672, ErrorCategory::ProviderRateLimit),
        (5, ErrorCategory::ProviderUnavailable),
        (99, ErrorCategory::ReauthorizationRequired),
        (22, ErrorCategory::ReauthorizationRequired),
        (2111, ErrorCategory::ReauthorizationRequired),
        (2123, ErrorCategory::ReauthorizationRequired),
        (2124, ErrorCategory::ReauthorizationRequired),
        (2101, ErrorCategory::ConfigurationError),
        (2102, ErrorCategory::ConfigurationError),
        (51, ErrorCategory::ProviderValidation),
        (999, ErrorCategory::ProviderValidation),
    ] {
        let bytes =
            serde_json::json!({"ErrorCode":code, "ThrottleSeconds":7, "ErrorStatus":"Success"})
                .to_string()
                .into_bytes();
        assert_eq!(
            decode_profile(
                &bytes,
                &membership,
                DefinitionVersion::new("v1"),
                Timestamp::new(1)
            )
            .unwrap_err()
            .category(),
            category,
            "profile code {code}"
        );
        assert_eq!(
            decode_effective_membership(&bytes).unwrap_err().category(),
            category,
            "membership code {code}"
        );
        assert_eq!(
            destiny_adapters::manifest::decode_manifest_index(&bytes)
                .unwrap_err()
                .category(),
            category,
            "manifest code {code}"
        );
    }
}
