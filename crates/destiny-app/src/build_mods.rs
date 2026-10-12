//! Request-local catalog assembly for the bounded DIM mod-assignment slice.
use destiny_core::{
    armor_catalog::*,
    build::BuildDefinition,
    build_analysis::{BuildAnalysis, FindingKind},
    ports::WorldDefinitionRepository,
    *,
};
use std::collections::BTreeSet;

pub(crate) fn findings(build: &BuildDefinition, analysis: &mut BuildAnalysis) {
    let artifact = build
        .parameters
        .as_ref()
        .is_some_and(|p| p.artifact_unlocks.is_some());
    let needs_stats = build
        .parameters
        .as_ref()
        .and_then(|p| p.stat_constraints.as_ref())
        .is_some_and(|cs| cs.iter().any(|c| c.min_stat.is_some_and(|min| min > 0)));
    for finding in &mut analysis.findings {
        let value = match finding.kind {
            FindingKind::InvalidMods if artifact => Some(true),
            FindingKind::InvalidMods => analysis
                .mod_fit
                .invalid_mods
                .as_ref()
                .map(|mods| !mods.is_empty()),
            FindingKind::ModsDontFit => analysis
                .mod_fit
                .unassigned_mods
                .as_ref()
                .map(|mods| !mods.is_empty()),
            FindingKind::NeedsArmorUpgrades => analysis
                .mod_fit
                .needs_upgrades_for_mods
                .filter(|needed| *needed || !needs_stats),
            _ => continue,
        };
        finding.present = value;
        finding.evaluation = if value.is_some() {
            "evaluated"
        } else {
            "not_evaluated"
        };
        finding.reason = if finding.kind == FindingKind::InvalidMods && artifact {
            Some("artifact_unlocks_present")
        } else if finding.kind == FindingKind::NeedsArmorUpgrades && value.is_none() && needs_stats
        {
            Some("dim_stat_upgrade_rules_not_implemented")
        } else if value.is_none() {
            analysis.mod_fit.reason
        } else {
            None
        };
    }
}

pub(crate) fn catalogs<R: WorldDefinitionRepository + ArmorCatalogRepository>(
    repository: &R,
    definitions: &mut Vec<ItemDefinition>,
) -> Result<Vec<ArmorCatalogRecord>, AppError> {
    let entries: Vec<_> = definitions
        .iter()
        .filter(|d| {
            d.bucket_hash
                .is_some_and(|b| mod_assignment::ARMOR_BUCKETS.contains(&b.value()))
        })
        .flat_map(|d| {
            d.armor
                .socket_categories
                .iter()
                .filter(|c| c.socket_category_hash == 590099826)
                .flat_map(|c| {
                    c.socket_indexes
                        .iter()
                        .filter_map(|i| d.armor.socket_entries.get(*i as usize))
                })
        })
        .cloned()
        .collect();
    let mut records = Vec::new();
    let mut keys = BTreeSet::new();
    for entry in entries {
        for (kind, hash) in [
            (ArmorCatalogKind::SocketType, Some(entry.socket_type_hash)),
            (
                ArmorCatalogKind::PlugSet,
                entry
                    .reusable_plug_set_hash
                    .or(entry.randomized_plug_set_hash),
            ),
        ] {
            if let Some(hash) = hash
                && keys.insert((kind.id(), hash))
                && let Some(record) = repository.lookup_armor_catalog(kind, hash)?
            {
                records.push(record);
            }
        }
    }
    let category_hashes: BTreeSet<_> = records
        .iter()
        .filter_map(|r| match r {
            ArmorCatalogRecord::SocketType(t) => Some(t.socket_category_hash),
            _ => None,
        })
        .collect();
    for hash in category_hashes {
        if let Some(record) =
            repository.lookup_armor_catalog(ArmorCatalogKind::SocketCategory, hash)?
        {
            records.push(record);
        }
    }
    let hashes: BTreeSet<_> = records
        .iter()
        .filter_map(|r| match r {
            ArmorCatalogRecord::PlugSet(s) => Some(&s.reusable_plug_items),
            _ => None,
        })
        .flatten()
        .map(|p| ItemHash::new(i64::from(p.plug_item_hash)))
        .collect();
    for hash in hashes {
        if !definitions.iter().any(|d| d.hash == hash)
            && let Resolution::Unique(d) = repository.lookup_definition(hash)?
        {
            definitions.push(d);
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::{build::*, build_analysis::analyze_build};

    #[test]
    fn findings_distinguish_mod_upgrades_from_unimplemented_stat_upgrades() {
        for (mods_need_upgrade, stat_min, expected) in [
            (false, 0, Some(false)),
            (false, 10, None),
            (true, 10, Some(true)),
        ] {
            let build = BuildDefinition {
                parameters: Some(BuildParameters {
                    stat_constraints: Some(vec![StatConstraint {
                        stat_hash: 1,
                        min_stat: Some(stat_min),
                        max_stat: None,
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let mut analysis =
                analyze_build(&build, &CharacterId::new("1"), &[], &[], &[], &[], &[]);
            analysis.mod_fit = mod_assignment::ModFitAnalysis {
                evaluation: "evaluated",
                reason: None,
                invalid_mods: Some(vec![42]),
                unassigned_mods: Some(vec![]),
                needs_upgrades_for_mods: Some(mods_need_upgrade),
            };
            findings(&build, &mut analysis);
            let present = |kind| {
                analysis
                    .findings
                    .iter()
                    .find(|f| f.kind == kind)
                    .unwrap()
                    .present
            };
            assert_eq!(present(FindingKind::InvalidMods), Some(true));
            assert_eq!(present(FindingKind::ModsDontFit), Some(false));
            assert_eq!(present(FindingKind::NeedsArmorUpgrades), expected);
        }
    }
}
