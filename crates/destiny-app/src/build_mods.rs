//! Request-local catalog assembly for the bounded DIM mod-assignment slice.
use destiny_core::{
    armor_catalog::*,
    build::BuildDefinition,
    build_analysis::{BuildAnalysis, FindingKind},
    ports::WorldDefinitionRepository,
    *,
};
use std::collections::BTreeSet;

/// DIM selectors.ts:333-346 and records/plugset-helpers.ts:26-36.
/// Cost-pair armor mods use enabled, not canInsert. The ornament and Harmonic
/// Resistance exceptions do not include any member of the pinned cost table.
pub(crate) fn unlocked_mods(entries: &[domain::PlugSetEntry], target: &CharacterId) -> Vec<u32> {
    entries
        .iter()
        .filter(|entry| {
            entry.enabled
                && match &entry.scope {
                    domain::PlugSetScope::Profile => true,
                    domain::PlugSetScope::Character(id) => id == target,
                }
        })
        .map(|entry| entry.plug_hash.value() as u32)
        .collect()
}

/// DIM selectors.ts:348-359: every plug of the tuning plug set counts as unlocked, because
/// tuning mods never appear in profile plug sets. Without the stored catalog nothing is added.
pub(crate) fn unlocked_with_tuning<R: ArmorCatalogRepository>(
    repository: &R,
    entries: &[domain::PlugSetEntry],
    target: &CharacterId,
) -> Result<Vec<u32>, AppError> {
    let mut unlocked = unlocked_mods(entries, target);
    if let Some(ArmorCatalogRecord::PlugSet(set)) = repository
        .lookup_armor_catalog(ArmorCatalogKind::PlugSet, optimizer::TUNING_SOCKET_PLUG_SET)?
    {
        unlocked.extend(set.reusable_plug_items.iter().map(|i| i.plug_item_hash));
    }
    Ok(unlocked)
}

/// Plugs the plug-set entries of the target (or the profile) report as insertable.
pub(crate) fn insertable_plugs(entries: &[domain::PlugSetEntry], target: &CharacterId) -> Vec<u32> {
    entries
        .iter()
        .filter(|entry| {
            entry.can_insert
                && match &entry.scope {
                    domain::PlugSetScope::Profile => true,
                    domain::PlugSetScope::Character(id) => id == target,
                }
        })
        .map(|entry| entry.plug_hash.value() as u32)
        .collect()
}

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
            FindingKind::UsesSeasonalMods => analysis.mod_fit.uses_seasonal_mods,
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
    items: &[ItemInstance],
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
                .filter(|c| [590099826, 760375309, 446880883].contains(&c.socket_category_hash))
                .flat_map(|c| {
                    c.socket_indexes
                        .iter()
                        .filter_map(|i| d.armor.socket_entries.get(*i as usize))
                })
        })
        .cloned()
        .collect();
    let mut records = Vec::new();
    // Nameless stat-roll plugs are not mods; core ignores the ones the store knows.
    let mut stat_plug_keys = BTreeSet::new();
    let armor = items.iter().filter(|item| {
        definitions.iter().any(|d| {
            d.hash == item.item_hash
                && d.bucket_hash
                    .is_some_and(|b| mod_assignment::ARMOR_BUCKETS.contains(&b.value()))
        })
    });
    for item in armor {
        for plug in crate::armor_model::resolve_stat_plugs(repository, item, definitions)? {
            if stat_plug_keys.insert(plug.hash) {
                records.push(ArmorCatalogRecord::StatPlug(plug));
            }
        }
    }
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
    if !definitions.iter().any(|d| d.hash.value() == 720825311)
        && let Resolution::Unique(d) = repository.lookup_definition(ItemHash::new(720825311))?
    {
        definitions.push(d);
    }
    let requirements: BTreeSet<_> = definitions
        .iter()
        .filter_map(|d| d.armor.insertion_material_requirement_hash)
        .collect();
    for hash in requirements {
        if let Some(record) =
            repository.lookup_armor_catalog(ArmorCatalogKind::MaterialRequirementSet, hash)?
        {
            records.push(record);
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::{build::*, build_analysis::analyze_build};

    #[test]
    fn assembly_loads_tier_plugs_materials_and_exotic_enhancement() {
        use crate::test_support::{InMemoryDefinitionRepository, synthetic_world};
        use destiny_core::ports::{DefinitionReplacementInput, ManifestDefinitions};
        let version = DefinitionVersion::new("v1");
        let mut armor = ItemDefinition {
            hash: ItemHash::new(1),
            name: "Armor".into(),
            bucket_hash: Some(BucketHash::new(mod_assignment::ARMOR_BUCKETS[0])),
            tier: Some("Legendary".into()),
            item_type: None,
            element: None,
            slot: None,
            class_type: None,
            plug_category: None,
            tags: vec![],
            version: version.clone(),
            armor: Default::default(),
            non_transferrable: None,
            is_instance_item: Some(true),
            does_postmaster_pull_have_side_effects: None,
            max_stack_size: None,
            stack_unique_label: None,
        };
        armor.armor.socket_entries.push(DefinitionSocketEntry {
            socket_type_hash: 99,
            reusable_plug_set_hash: Some(201),
            ..Default::default()
        });
        armor
            .armor
            .socket_categories
            .push(DefinitionSocketCategory {
                socket_category_hash: 760375309,
                socket_indexes: vec![0],
            });
        let mut plug = armor.clone();
        plug.hash = ItemHash::new(2);
        plug.bucket_hash = None;
        plug.armor = ArmorDefinition {
            plug_category_hash: Some(400813043),
            insertion_material_requirement_hash: Some(301),
            ..Default::default()
        };
        let mut enhancement = plug.clone();
        enhancement.hash = ItemHash::new(720825311);
        enhancement.armor.insertion_material_requirement_hash = Some(302);
        let mut world = synthetic_world(version.clone());
        world.armor_catalogs = vec![
            ArmorCatalogRecord::SocketType(SocketTypeDefinition {
                hash: 99,
                socket_category_hash: 100,
                always_randomize_sockets: false,
                plug_whitelist: vec![],
            }),
            ArmorCatalogRecord::SocketCategory(SocketCategoryDefinition {
                hash: 100,
                category_style: 3,
                name: "Tier".into(),
            }),
            ArmorCatalogRecord::PlugSet(PlugSetDefinition {
                hash: 201,
                reusable_plug_items: vec![PlugSetItem {
                    plug_item_hash: 2,
                    currently_can_roll: false,
                }],
            }),
            ArmorCatalogRecord::MaterialRequirementSet(MaterialRequirementSetDefinition {
                hash: 301,
                materials: vec![],
            }),
            ArmorCatalogRecord::MaterialRequirementSet(MaterialRequirementSetDefinition {
                hash: 302,
                materials: vec![],
            }),
        ];
        let mut repository = InMemoryDefinitionRepository::default();
        repository
            .publish_manifest(
                DefinitionReplacementInput {
                    definitions: ManifestDefinitions {
                        version,
                        definitions: vec![armor.clone(), plug, enhancement],
                    },
                    checked_at: Timestamp::new(0),
                    source: SourceMarker::Manifest,
                },
                world,
            )
            .unwrap();
        let mut definitions = vec![armor];
        let records = catalogs(&repository, &mut definitions, &[]).unwrap();
        assert_eq!(
            definitions
                .iter()
                .map(|d| d.hash.value())
                .collect::<Vec<_>>(),
            vec![1, 2, 720825311]
        );
        assert_eq!(records.len(), 5);
        for hash in [301, 302] {
            assert!(records.iter().any(
                |r| matches!(r, ArmorCatalogRecord::MaterialRequirementSet(s) if s.hash == hash)
            ));
        }
    }

    #[test]
    fn mod_unlocks_are_enabled_and_target_scoped_not_insertable() {
        let entries = [
            (domain::PlugSetScope::Profile, 1, true, false),
            (
                domain::PlugSetScope::Character(CharacterId::new("target")),
                2,
                true,
                false,
            ),
            (
                domain::PlugSetScope::Character(CharacterId::new("other")),
                3,
                true,
                true,
            ),
            (domain::PlugSetScope::Profile, 4, false, true),
        ]
        .map(|(scope, hash, enabled, can_insert)| domain::PlugSetEntry {
            scope,
            plug_hash: ItemHash::new(hash),
            enabled,
            can_insert,
        });
        assert_eq!(
            unlocked_mods(&entries, &CharacterId::new("target")),
            vec![1, 2]
        );
        assert!(unlocked_mods(&[], &CharacterId::new("target")).is_empty());
    }

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
                uses_seasonal_mods: Some(true),
                ..Default::default()
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
            assert_eq!(present(FindingKind::UsesSeasonalMods), Some(true));
            assert_eq!(present(FindingKind::NeedsArmorUpgrades), expected);
        }
    }
}
