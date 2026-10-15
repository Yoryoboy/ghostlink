//! Shared inspect wish-list views; `none` means no list has an opinion.
use destiny_app::wishlist::WishlistInspection;
use serde_json::{Value, json};

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// The same JSON object for CLI `--json` and the MCP `wishlist` field.
pub fn wishlist_json(inspection: &WishlistInspection) -> Value {
    json!({
        "verdict": inspection.verdict.as_str(),
        "matches_total": inspection.matches_total,
        "matches": inspection.matches.iter().map(|matched| json!({
            "source": matched.source,
            "section": matched.section,
            "perks": matched.perks,
            "notes": matched.notes,
            "tags": matched.tags,
        })).collect::<Vec<_>>(),
    })
}

/// One summary line, then one line per cited match with empty parts omitted.
pub fn wishlist_text(inspection: &WishlistInspection) -> Vec<String> {
    let mut lines = vec![format!(
        "wishlist: {} ({} matching)",
        inspection.verdict.as_str(),
        inspection.matches_total
    )];
    for matched in &inspection.matches {
        let mut parts = vec![sanitize(&matched.source)];
        parts.extend(matched.section.as_deref().map(sanitize));
        if !matched.perks.is_empty() {
            parts.push(format!("perks: {}", sanitize(&matched.perks.join(", "))));
        }
        if let Some(notes) = &matched.notes {
            parts.push(format!("notes: {}", sanitize(notes)));
        }
        if !matched.tags.is_empty() {
            parts.push(format!("tags: {}", sanitize(&matched.tags.join(", "))));
        }
        lines.push(format!("wishlist match: {}", parts.join(" | ")));
    }
    lines
}
