//! Request-local, read-only armor derivation from committed instance/Manifest data.
use destiny_core::domain::ItemSlot;
use destiny_core::{armor_catalog::*, armor_model::*, *};

pub fn inspect_armor_model<R>(
    repository: &R,
    definition: &ItemDefinition,
    instance: &ItemInstance,
) -> Result<Option<ArmorModel>, AppError>
where
    R: DefinitionRepository + ArmorCatalogRepository + StatDefinitionRepository,
{
    if !matches!(
        definition.slot,
        Some(
            ItemSlot::Helmet
                | ItemSlot::Gauntlets
                | ItemSlot::Chest
                | ItemSlot::Legs
                | ItemSlot::ClassItem
        )
    ) {
        return Ok(None);
    }
    let hashes: Vec<_> = instance
        .sockets
        .iter()
        .filter_map(|socket| socket.plug_hash)
        .collect();
    let definitions = repository.lookup_definition_batch(&hashes)?;
    let groups = repository.armor_catalog_batch(
        ArmorCatalogKind::StatGroup,
        &definition
            .armor
            .stat_group_hash
            .into_iter()
            .collect::<Vec<_>>(),
    )?;
    let group = groups.iter().find_map(|record| match record {
        ArmorCatalogRecord::StatGroup(group) => Some(group),
        _ => None,
    });
    // Empty legacy socket projections cannot establish that intrinsic rolls are zero.
    let plugs: Vec<_> = instance
        .sockets
        .iter()
        .filter_map(|socket| {
            socket.plug_hash.map(|hash| ModelPlug {
                hash: hash.value() as u32,
                enabled: socket.enabled,
                definition: definitions
                    .iter()
                    .find(|definition| definition.hash == hash)
                    .map(|definition| &definition.armor),
            })
        })
        .collect();
    let known_sockets = (!instance.sockets.is_empty()).then_some(plugs.as_slice());
    let mut known_stats = true;
    for hash in ARMOR_STATS {
        known_stats &= repository.lookup_stat(hash)?.is_some();
    }
    let stats = if known_stats {
        build_armor_stats(
            Some(&definition.armor),
            group,
            known_sockets,
            instance.energy.is_some(),
        )
    } else {
        None
    };
    let masterwork_tier = armor3_masterwork_tier(known_sockets, stats.as_ref());
    Ok(Some(ArmorModel {
        stats,
        masterwork_tier,
        gear_tier: instance.gear_tier,
        // DIM inventory/store/d2-item-factory.ts:586 consumer-only fallback.
        effective_gear_tier: instance.gear_tier.unwrap_or(0),
        energy: instance.energy.clone(),
    }))
}
