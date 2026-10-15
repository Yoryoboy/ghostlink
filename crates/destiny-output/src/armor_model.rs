//! Shared additive inspect armor JSON. Null denotes unknown, not a zero roll.
use destiny_app::ArmorModel;
use serde_json::{Value, json};

/// English, numeric-only presentation; unknown values never masquerade as zero.
pub fn armor_model_text(model: &ArmorModel) -> Vec<String> {
    let known =
        |value: Option<i32>| value.map_or_else(|| "unknown".to_owned(), |value| value.to_string());
    let mut lines = vec![
        "armor model:".to_owned(),
        format!(
            "  total base: {}",
            known(model.stats.as_ref().map(|stats| stats.total_base))
        ),
        format!("  masterwork tier: {}", known(model.masterwork_tier)),
        format!(
            "  gear tier: {} (effective {})",
            model
                .gear_tier
                .map_or_else(|| "unknown".to_owned(), |tier| tier.to_string()),
            model.effective_gear_tier
        ),
    ];
    if let Some(stats) = &model.stats {
        for stat in &stats.stats {
            lines.push(format!(
                "  stat {}: base {}, current {}, assumed masterworked {}, maximum {}",
                stat.stat_hash,
                stat.base,
                stat.current,
                known(stat.base_masterworked),
                known(stat.maximum_value),
            ));
        }
    } else {
        lines.push("  base stats: unknown".to_owned());
    }
    lines.push(model.energy.as_ref().map_or_else(
        || "  energy: unknown".to_owned(),
        |energy| {
            format!(
                "  energy: capacity {}, used {}, unused {}",
                energy.energy_capacity, energy.energy_used, energy.energy_unused
            )
        },
    ));
    lines.push(model.tuning.as_ref().map_or_else(
        || "  tuning socket: unknown or absent".to_owned(),
        |tuning| {
            format!(
                "  tuning socket: {}, plug {}; tuned stat: {}",
                tuning.socket_index,
                tuning.plug_hash,
                tuning
                    .tuned_stat_hash
                    .map_or_else(|| "unknown".to_owned(), |hash| hash.to_string())
            )
        },
    ));
    if let Some(set) = &model.armor_set {
        lines.push(format!(
            "  armor set: {} ({})",
            set.hash,
            set.name
                .as_deref()
                .unwrap_or("unknown")
                .chars()
                .filter(|character| !character.is_control())
                .collect::<String>()
        ));
        if let Some(perks) = &set.perks {
            for perk in perks {
                lines.push(format!(
                    "    perk {}: requires {} pieces; name: {}; icon: {}",
                    perk.sandbox_perk_hash,
                    perk.required_set_count,
                    safe_text(perk.name.as_deref()),
                    safe_text(perk.icon.as_deref())
                ));
            }
        } else {
            lines.push("    set perks: unknown".to_owned());
        }
    } else {
        lines.push("  armor set: unknown or absent".to_owned());
    }
    lines
}

fn safe_text(value: Option<&str>) -> String {
    value
        .unwrap_or("unknown")
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

pub fn fragment_capacity_json(model: &destiny_app::FragmentCapacity) -> Value {
    json!({ "capacity": model.capacity })
}

pub fn fragment_capacity_text(model: &destiny_app::FragmentCapacity) -> String {
    format!(
        "fragment capacity: {}",
        model
            .capacity
            .map_or_else(|| "unknown".to_owned(), |value| value.to_string())
    )
}

pub fn armor_model_json(model: &ArmorModel) -> Value {
    json!({
        "tuning": model.tuning.as_ref().map(|tuning| json!({
            "socket_index": tuning.socket_index, "plug_hash": tuning.plug_hash,
            "tuned_stat_hash": tuning.tuned_stat_hash,
            "reusable_membership_known": tuning.reusable_membership_known,
            "tuned_stat_not_evaluated_reason": if !tuning.reusable_membership_known { Some("raw_reusable_plugs_not_projected") } else if tuning.tuned_stat_hash.is_none() { Some("no_unique_directional_tuner") } else { None },
        })),
        "armor_set": model.armor_set.as_ref().map(|set| json!({
            "hash": set.hash, "name": set.name,
            "perks": set.perks.as_ref().map(|perks| perks.iter().map(|perk| json!({
                "sandbox_perk_hash": perk.sandbox_perk_hash, "required_set_count": perk.required_set_count,
                "name": perk.name, "icon": perk.icon,
            })).collect::<Vec<_>>()),
        })),
        "base_stats": model.stats.as_ref().map(|stats| stats.stats.iter().map(|stat| json!({
            "stat_hash": stat.stat_hash, "base": stat.base, "current": stat.current,
            "base_masterworked": stat.base_masterworked, "maximum_value": stat.maximum_value,
        })).collect::<Vec<_>>()),
        "total_base": model.stats.as_ref().map(|stats| stats.total_base),
        "masterwork_tier": model.masterwork_tier,
        "gear_tier": model.gear_tier,
        "effective_gear_tier": model.effective_gear_tier,
        "energy": model.energy.as_ref().map(|energy| json!({
            "capacity": energy.energy_capacity, "used": energy.energy_used,
            "unused": energy.energy_unused, "energy_type": energy.energy_type,
            "energy_type_hash": energy.energy_type_hash,
        })),
    })
}
