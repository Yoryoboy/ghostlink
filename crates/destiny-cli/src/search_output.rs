//! Line-oriented inventory tile rendering.
use serde_json::Value;

pub(crate) fn compact_line(hit: &Value) -> String {
    let text = |key: &str| super::sanitize(hit[key].as_str().unwrap_or("Unknown"));
    let mut parts = vec![format!("{} ({})", text("name"), text("instance_id"))];
    if let Some(power) = hit["power"].as_u64() {
        parts.push(power.to_string());
    }
    for key in ["tier", "element"] {
        if hit[key].is_string() {
            parts.push(text(key));
        }
    }
    let location = &hit["location"];
    let mut state = match location["kind"].as_str() {
        Some("vault") => "Vault".into(),
        Some("character") => format!(
            "Character {}",
            super::sanitize(location["character_id"].as_str().unwrap_or("Unknown"))
        ),
        Some("postmaster") => format!(
            "Postmaster {}",
            super::sanitize(location["character_id"].as_str().unwrap_or("Unknown"))
        ),
        _ => "Other".into(),
    };
    if hit["equipped"] == true {
        state.push_str(" [equipped]");
    }
    if hit["locked"] == true {
        state.push_str(" [locked]");
    }
    if let Some(quantity) = hit["quantity"].as_u64() {
        state.push_str(&format!(" ×{quantity}"));
    }
    parts.push(state);
    let names = |key: &str| {
        hit[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(super::sanitize)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let perks = names("perk_names");
    let mut line = parts.join(" · ");
    if !perks.is_empty() {
        line.push_str(&format!(" — {perks}"));
    }
    let mods = names("mods");
    if !mods.is_empty() {
        line.push_str(&format!(" · Mods: {mods}"));
    }
    for stat in hit["stats"].as_array().into_iter().flatten() {
        line.push_str(&format!(
            " · {} {}",
            super::sanitize(stat["name"].as_str().unwrap_or("Unknown")),
            stat["value"]
        ));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn tile_is_one_sanitized_line() {
        let hit = json!({"name":"Test Helmet", "instance_id":"123", "power":2010, "tier":"Legendary", "element":"Arc", "location":{"kind":"vault"}, "equipped":false, "locked":true, "perk_names":["Plugged Perk"], "mods":["Plugged Mod"], "stats":[{"name":"Mobility","value":30},{"name":"Recovery","value":20}]});
        let line = compact_line(&hit);
        assert_eq!(
            line,
            "Test Helmet (123) · 2010 · Legendary · Arc · Vault [locked] — Plugged Perk · Mods: Plugged Mod · Mobility 30 · Recovery 20"
        );
        let mut hostile = hit;
        hostile["name"] = json!("Fake\nLine\u{1b}");
        assert!(!compact_line(&hostile).contains(['\n', '\u{1b}']));
    }
}
