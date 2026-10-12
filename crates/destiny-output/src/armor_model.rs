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
    lines
}

pub fn armor_model_json(model: &ArmorModel) -> Value {
    json!({
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
