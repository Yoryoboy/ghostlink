//! Request-local DIM stat evaluation. No provider requests or optimizer claims.
use destiny_core::{
    armor_catalog::*, armor_model::*, build::*, build_analysis::*, build_stats::*, *,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn evaluate<
    R: DefinitionRepository + StatDefinitionRepository + ArmorCatalogRepository,
>(
    repository: &R,
    build: &BuildDefinition,
    analysis: &mut BuildAnalysis,
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    unlocked: &[u32],
    class: u8,
) -> Result<(), AppError> {
    let result = assemble(
        repository,
        build,
        analysis,
        definitions,
        catalogs,
        unlocked,
        class,
    )?;
    let constraints = build
        .parameters
        .as_ref()
        .and_then(|p| p.stat_constraints.as_deref())
        .unwrap_or(&[]);
    let trivial = !ARMOR_STATS.iter().any(|hash| {
        constraints
            .iter()
            .rev()
            .find(|c| c.stat_hash == *hash)
            .is_some_and(|c| c.min_stat.unwrap_or(0) > 0)
    });
    for finding in &mut analysis.findings {
        let present = match finding.kind {
            FindingKind::DoesNotSatisfyStatConstraints => result.satisfies_constraints.map(|v| !v),
            FindingKind::NeedsArmorUpgrades => {
                let mods = analysis.mod_fit.needs_upgrades_for_mods;
                let stats = result.needs_upgrades_for_stats.or(trivial.then_some(false));
                if mods == Some(true) || stats == Some(true) {
                    Some(true)
                } else if mods == Some(false) && stats == Some(false) {
                    Some(false)
                } else {
                    None
                }
            }
            _ => continue,
        };
        finding.present = present;
        finding.evaluation = if present.is_some() {
            "evaluated"
        } else {
            "not_evaluated"
        };
        finding.reason = if present.is_some() {
            None
        } else {
            result.reason.or(analysis.mod_fit.reason)
        };
    }
    analysis.stats = result;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn assemble<R: DefinitionRepository + StatDefinitionRepository + ArmorCatalogRepository>(
    repository: &R,
    build: &BuildDefinition,
    analysis: &BuildAnalysis,
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    unlocked: &[u32],
    class: u8,
) -> Result<BuildStats, AppError> {
    let armor: Vec<_> = analysis
        .items
        .iter()
        .filter(|r| r.should_equip)
        .filter_map(|r| r.inventory.as_ref())
        .filter_map(|i| {
            definitions
                .iter()
                .find(|d| d.hash == i.item_hash)
                .map(|d| (i, d))
        })
        .filter(|(_, d)| {
            d.bucket_hash
                .is_some_and(|b| mod_assignment::ARMOR_BUCKETS.contains(&b.value()))
        })
        .collect();
    if armor.len() != 5 {
        return Ok(BuildStats::unknown("full_resolved_armor_set_required"));
    }
    let mut models = Vec::new();
    let mut masterworks = Vec::new();
    for (item, definition) in &armor {
        let Some(model) = crate::armor_model::inspect_armor_model(repository, definition, item)?
        else {
            return Ok(BuildStats::unknown("armor_base_stats_unavailable"));
        };
        let Some(stats) = model.stats.as_ref() else {
            return Ok(BuildStats::unknown("armor_base_stats_unavailable"));
        };
        let mut mw = None;
        for socket in &item.sockets {
            let Some(hash) = socket.plug_hash else {
                continue;
            };
            let Some(plug) = definitions.iter().find(|d| d.hash == hash) else {
                return Ok(BuildStats::unknown("inserted_plug_definition_unavailable"));
            };
            if plug.armor.plug_category_hash == Some(ARMOR3_MASTERWORK_PLUG) && mw.is_none() {
                let mut values = [0; 6];
                let mut base = [0; 6];
                for (i, hash) in ARMOR_STATS.iter().enumerate() {
                    let Some(stat) = stats.stats.iter().find(|s| s.stat_hash == *hash) else {
                        return Ok(BuildStats::unknown("armor_base_stats_unavailable"));
                    };
                    base[i] = stat.base;
                }
                if let Err(reason) = add_plug_stats(
                    &mut values,
                    hash.value() as u32,
                    &plug.armor,
                    class,
                    Some(&base),
                ) {
                    return Ok(BuildStats::unknown(reason));
                }
                mw = Some(values);
            }
        }
        masterworks.push(mw);
        models.push(model);
    }
    let armor: Vec<_> = armor
        .iter()
        .zip(&models)
        .zip(masterworks)
        .map(|(((item, definition), model), masterwork)| StatArmor {
            item,
            definition,
            stats: model.stats.as_ref().expect("checked stats"),
            masterwork,
        })
        .collect();
    let subclass = analysis.items.iter().find_map(|r| {
        let item = r.inventory.as_ref()?;
        definitions
            .iter()
            .find(|d| {
                d.hash == item.item_hash && d.bucket_hash == Some(BucketHash::new(3284755031))
            })
            .map(|_| r)
    });
    // getTotalModStatChanges uses every saved override, not current sockets or
    // only fragments. A known non-pluggable definition is omitted by DIM.
    let mut plugs = Vec::new();
    if let Some(subclass) = subclass {
        for (_, hash) in &subclass.spec.socket_overrides {
            let Some(plug) = definitions.iter().find(|d| d.hash == *hash) else {
                return Ok(BuildStats::unknown(
                    "subclass_stat_plug_definition_unavailable",
                ));
            };
            if plug.armor.plug_category_hash.is_some() {
                plugs.push(hash.value() as u32);
            }
        }
    }
    let runtime = build
        .parameters
        .as_ref()
        .and_then(|p| p.include_runtime_stat_benefits)
        .unwrap_or(false);
    let assumption = inferred_masterwork(build, &armor);
    let as_is_fit = mod_assignment::stat_mod_assignments(
        build,
        &analysis.items,
        definitions,
        catalogs,
        unlocked,
        false,
    );
    let assumed_fit = mod_assignment::stat_mod_assignments(
        build,
        &analysis.items,
        definitions,
        catalogs,
        unlocked,
        true,
    );
    let as_is = match total_stats(
        &armor,
        &as_is_fit,
        definitions,
        &plugs,
        class,
        runtime,
        None,
    ) {
        Ok(v) => v,
        Err(reason) => return Ok(BuildStats::unknown(reason)),
    };
    let assumed = match total_stats(
        &armor,
        &assumed_fit,
        definitions,
        &plugs,
        class,
        runtime,
        Some(assumption),
    ) {
        Ok(v) => v,
        Err(reason) => return Ok(BuildStats::unknown(reason)),
    };
    let constraints = build
        .parameters
        .as_ref()
        .and_then(|p| p.stat_constraints.as_deref())
        .unwrap_or(&[]);
    let satisfies = satisfies_minima(&assumed, constraints);
    Ok(BuildStats {
        evaluation: "evaluated",
        reason: None,
        as_is: Some(as_is),
        assumed: Some(assumed),
        satisfies_constraints: Some(satisfies),
        needs_upgrades_for_stats: Some(satisfies && !satisfies_minima(&as_is, constraints)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{InMemoryDefinitionRepository, synthetic_world};
    use destiny_core::domain::ItemSlot;
    use destiny_core::ports::{
        DefinitionReplacementInput, ManifestDefinitions, WorldDefinitionRepository,
    };

    #[test]
    #[rustfmt::skip] // Keep the synthetic armor/catalog fixture compact.
    fn stat_only_upgrades_and_unknown_inputs_reach_findings_without_optimizer() {
        let version = DefinitionVersion::new("v1");
        let base = ItemDefinition {
            hash: ItemHash::new(1),
            name: "Armor".into(),
            bucket_hash: None,
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
        let mut plug = base.clone();
        plug.hash = ItemHash::new(42);
        plug.armor.plug_category_hash = Some(2912171003);
        let mut mw = base.clone();
        mw.hash = ItemHash::new(43);
        mw.armor.plug_category_hash = Some(ARMOR3_MASTERWORK_PLUG);
        mw.armor.investment_stats = ARMOR_STATS.iter().map(|hash| InvestmentStat {
            stat_type_hash: *hash,
            value: 2,
            is_conditionally_active: true,
        }).collect();
        let mut definitions = vec![plug, mw];
        let mut inventory = Vec::new();
        let mut build = BuildDefinition {
            parameters: Some(BuildParameters {
                assume_armor_masterwork: Some(AssumeArmorMasterwork::All),
                mods: Some(vec![ItemHash::new(42)]),
                stat_constraints: Some(vec![StatConstraint {
                    stat_hash: ARMOR_STATS[3],
                    min_stat: Some(20),
                    max_stat: Some(0),
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let slots = [ItemSlot::Helmet, ItemSlot::Gauntlets, ItemSlot::Chest, ItemSlot::Legs, ItemSlot::ClassItem];
        for (index, bucket) in mod_assignment::ARMOR_BUCKETS.iter().enumerate() {
            let mut definition = base.clone();
            definition.hash = ItemHash::new(index as i64 + 1);
            definition.bucket_hash = Some(BucketHash::new(*bucket));
            definition.slot = Some(slots[index]);
            definition.armor.stat_group_hash = Some(100);
            definition.armor.investment_stats = ARMOR_STATS.iter().enumerate().map(|(i, hash)| InvestmentStat {
                stat_type_hash: *hash,
                value: if i < 3 { 10 } else { 0 },
                is_conditionally_active: false,
            }).collect();
            definition.armor.socket_categories = vec![DefinitionSocketCategory {
                socket_category_hash: 590099826,
                socket_indexes: vec![0],
            }];
            definition.armor.socket_entries = vec![DefinitionSocketEntry {
                socket_type_hash: 99,
                plug_sources: Some(2),
                ..Default::default()
            }];
            let item = ItemInstance {
                instance_id: ItemInstanceId::new((index + 1).to_string()),
                item_hash: definition.hash,
                location: ItemLocation::Vault,
                quantity: 1,
                equipped: false,
                locked: false,
                power: None,
                perks: vec![],
                plugs: vec![],
                stats: vec![],
                available_plugs: vec![],
                energy: Some(ItemEnergy { energy_capacity: 7, ..Default::default() }),
                gear_tier: Some(5),
                transfer_status: Some(0),
                sockets: [42, 43].iter().enumerate().map(|(index, hash)| IndexedSocket {
                    socket_index: index as u8,
                    plug_hash: Some(ItemHash::new(*hash)),
                    enabled: false,
                    visible: true,
                    plug_objectives: None,
                    reusable_plugs: Some(vec![domain::ReusablePlug {
                        plug_hash: ItemHash::new(*hash),
                        enabled: false,
                        can_insert: false,
                    }]),
                }).collect(),
            };
            build.equipped.push(BuildItem {
                instance_id: Some(item.instance_id.clone()),
                hash: item.item_hash,
                amount: None,
                socket_overrides: vec![],
                crafted_date: None,
            });
            inventory.push(item);
            definitions.push(definition);
        }
        let catalogs = vec![
            ArmorCatalogRecord::SocketType(SocketTypeDefinition {
                hash: 99,
                socket_category_hash: 101,
                always_randomize_sockets: false,
                plug_whitelist: vec![],
            }),
            ArmorCatalogRecord::SocketCategory(SocketCategoryDefinition {
                hash: 101,
                category_style: 3,
                name: "Mods".into(),
            }),
            ArmorCatalogRecord::StatGroup(StatGroupDefinition {
                hash: 100,
                maximum_value: Some(200),
                scaled_stats: ARMOR_STATS.iter().map(|hash| ScaledStat {
                    stat_hash: *hash,
                    maximum_value: 200,
                    display_as_numeric: true,
                    display_interpolation: vec![],
                }).collect(),
            }),
        ];
        let mut world = synthetic_world(version.clone());
        world.stats.definitions = ARMOR_STATS.iter().map(|hash| StatDefinition::new(*hash, hash.to_string()).unwrap()).collect();
        world.armor_catalogs = catalogs.clone();
        let mut repository = InMemoryDefinitionRepository::default();
        repository.publish_manifest(DefinitionReplacementInput {
            definitions: ManifestDefinitions { version, definitions: definitions.clone() },
            checked_at: Timestamp::new(0),
            source: SourceMarker::Manifest,
        }, world).unwrap();
        let mut analysis = analyze_build(&build, &CharacterId::new("1"), &inventory, &definitions, &[], &[], &[]);
        analysis.mod_fit = mod_assignment::analyze_bucket_mods(&build, &analysis.items, &inventory, &definitions, &catalogs);
        assert_eq!(analysis.mod_fit.needs_upgrades_for_mods, Some(false));
        evaluate(&repository, &build, &mut analysis, &definitions, &catalogs, &[], 0).unwrap();
        assert_eq!(analysis.stats.as_is, Some([50, 50, 50, 10, 10, 10]));
        assert_eq!(analysis.stats.assumed, Some([50, 50, 50, 25, 25, 25]));
        assert_eq!(analysis.stats.needs_upgrades_for_stats, Some(true));
        let finding = |analysis: &BuildAnalysis, kind| analysis.findings.iter().find(|f| f.kind == kind).unwrap().present;
        assert_eq!(finding(&analysis, FindingKind::NeedsArmorUpgrades), Some(true));
        assert_eq!(finding(&analysis, FindingKind::DoesNotSatisfyStatConstraints), Some(false));
        assert_eq!(finding(&analysis, FindingKind::BetterStatsAvailable), None);
        let mut incomplete = definitions.clone();
        incomplete.retain(|d| d.hash != ItemHash::new(43));
        evaluate(&repository, &build, &mut analysis, &incomplete, &catalogs, &[], 0).unwrap();
        assert_eq!(analysis.stats.evaluation, "not_evaluated");
        assert_eq!(analysis.stats.reason, Some("inserted_plug_definition_unavailable"));
        assert_eq!(finding(&analysis, FindingKind::DoesNotSatisfyStatConstraints), None);
    }
}
