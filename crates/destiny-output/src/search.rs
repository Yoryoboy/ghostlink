//! Compact inventory tiles shared by both read transports.
use serde_json::Value;

/// Project a detailed hit using the query's resolved stat names.
pub fn compact_hit(hit: &Value, resolved: &Value) -> Value {
    let mut tile = serde_json::Map::new();
    for key in ["instance_id", "item_hash", "location", "equipped", "locked"] {
        tile.insert(key.into(), hit["instance"][key].clone());
    }
    // Unknown provider location text is not part of a compact tile in either transport.
    if tile["location"]["kind"] == "other" {
        tile.insert("location".into(), serde_json::json!({"kind": "other"}));
    }
    for key in ["name", "item_type", "tier"] {
        tile.insert(key.into(), hit["definition"][key].clone());
    }
    for (key, source) in [("element", &hit["definition"]), ("power", &hit["instance"])] {
        if !source[key].is_null() {
            tile.insert(key.into(), source[key].clone());
        }
    }
    if hit["instance"]["quantity"] != 1 {
        tile.insert("quantity".into(), hit["instance"]["quantity"].clone());
    }
    // These names were resolved from instance.plugs, not available_plugs.
    tile.insert("perk_names".into(), hit["perk_names"].clone());
    if hit["mods"].as_array().is_some_and(|mods| !mods.is_empty()) {
        tile.insert("mods".into(), hit["mods"].clone());
    }
    let referenced = |name: &Value| {
        name == &resolved["sort"]["name"]
            || resolved["stats"].as_object().is_some_and(|filters| {
                filters
                    .values()
                    .any(|names| names.as_array().is_some_and(|names| names.contains(name)))
            })
    };
    let stats: Vec<_> = hit["stats"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|stat| referenced(&stat["name"]))
        .cloned()
        .collect();
    if !stats.is_empty() {
        tile.insert("stats".into(), Value::Array(stats));
    }
    Value::Object(tile)
}

/// Replace only hits; candidates and all query metadata remain unchanged.
pub fn compact_search(mut output: Value) -> Value {
    output["hits"] = Value::Array(
        output["hits"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|hit| compact_hit(hit, &output["resolved"]))
            .collect(),
    );
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compact_tile_omits_detail_and_only_retains_referenced_stats() {
        let detail = json!({
            "definition": {"name":"Test Helmet", "item_type":"Helmet", "tier":"Legendary", "element":"Arc", "description":"Long description"},
            "instance": {"instance_id":"123", "item_hash":42, "location":{"kind":"vault"}, "power":2010, "equipped":false, "locked":true, "quantity":1, "perks":[{"perk_hash":99,"active":true}]},
            "perk_names":["Plugged Perk"], "mods":["Plugged Mod"],
            "available_perks":["Alternative Perk"], "available_mods":["Alternative Mod"], "archetype":"Heavy",
            "stats":[{"name":"Mobility","value":30},{"name":"Recovery","value":20},{"name":"Resilience","value":10}]
        });
        let resolved =
            json!({"stats":{"mobility":["Mobility"]},"sort":{"name":"Recovery","order":"desc"}});
        let compact = compact_hit(&detail, &resolved);
        assert_eq!(
            compact,
            json!({"instance_id":"123","item_hash":42,"name":"Test Helmet","item_type":"Helmet","tier":"Legendary","element":"Arc","power":2010,"location":{"kind":"vault"},"equipped":false,"locked":true,"perk_names":["Plugged Perk"],"mods":["Plugged Mod"],"stats":[{"name":"Mobility","value":30},{"name":"Recovery","value":20}]})
        );
        assert!(compact.to_string().len() * 3 < detail.to_string().len() * 2);
        println!(
            "compact={} detail={} bytes; hit={compact}",
            compact.to_string().len(),
            detail.to_string().len()
        );
        let mut sparse = detail.clone();
        sparse["definition"]["element"] = Value::Null;
        sparse["instance"]["power"] = Value::Null;
        sparse["instance"]["quantity"] = json!(2);
        sparse["mods"] = json!([]);
        let sparse = compact_hit(&sparse, &json!({}));
        for omitted in ["element", "power", "mods", "stats"] {
            assert!(sparse.get(omitted).is_none(), "{omitted}");
        }
        assert_eq!(sparse["quantity"], 2);
    }
}
