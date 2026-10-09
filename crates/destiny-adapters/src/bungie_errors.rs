//! Exact numeric PlatformErrorCodes only; never interpret provider text.
use destiny_core::ProviderRejection as R;
pub(crate) const fn rejection(code: u64) -> Option<R> {
    Some(match code {
        1623 => R::ItemNotFound,
        1632 => R::AlreadyInInventory,
        1634 | 1671 => R::NotInSocialSpace,
        1640 => R::Unequippable,
        1641 => R::ExoticLimit,
        1642 => R::DestinationFull,
        1648 => R::UniquenessViolation,
        1656 => R::Equipped,
        1660 => R::NotTransferable,
        1663 => R::ActionForbidden,
        1672 => R::Throttled,
        _ => return None,
    })
}

#[cfg(test)]
pub(crate) fn assert_action_codes(mut classify: impl FnMut(serde_json::Value) -> Option<R>) {
    use serde_json::json;
    for (code, reason) in [
        (1623, R::ItemNotFound),
        (1632, R::AlreadyInInventory),
        (1634, R::NotInSocialSpace),
        (1640, R::Unequippable),
        (1641, R::ExoticLimit),
        (1642, R::DestinationFull),
        (1648, R::UniquenessViolation),
        (1656, R::Equipped),
        (1660, R::NotTransferable),
        (1663, R::ActionForbidden),
        (1671, R::NotInSocialSpace),
        (1672, R::Throttled),
    ] {
        for status in ["Success", "DestinyItemNotFound", "untrusted"] {
            assert_eq!(
                classify(
                    json!({"Response":0,"ErrorCode":code,"ThrottleSeconds":0,"ErrorStatus":status})
                ),
                Some(reason),
                "code {code}"
            );
        }
    }
    for value in [
        json!({"Response":0,"ErrorCode":999,"ThrottleSeconds":0,"ErrorStatus":"DestinyItemNotFound"}),
        json!({"Response":0,"ThrottleSeconds":0,"ErrorStatus":"DestinyThrottledByGameServer"}),
    ] {
        assert_eq!(classify(value), None);
    }
}
