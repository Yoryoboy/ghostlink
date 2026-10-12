//! Transport-neutral presentation contracts shared by CLI and MCP.
pub mod apply_loadout;
pub mod armor_model;
pub mod build_apply_result;
pub mod builds;
pub mod equip;
pub mod equip_items;
pub mod instanced_pull;
pub mod loadouts;
pub mod lock;
pub mod move_item;
pub mod search;
pub mod stack_pull;

/// Actionable text for stable, sanitized provider rejection reasons.
pub fn rejection_message(reason: &str) -> Option<&'static str> {
    destiny_app::ProviderRejection::ALL
        .into_iter()
        .find(|candidate| candidate.reason() == reason)
        .map(destiny_app::ProviderRejection::message)
}

pub(crate) fn reason_text(reason: &str) -> &str {
    rejection_message(reason).unwrap_or(reason)
}
