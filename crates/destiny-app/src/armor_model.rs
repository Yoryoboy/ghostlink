//! Request-local, read-only armor derivation from committed instance/Manifest data.

pub fn inspect_fragment_capacity<R: DefinitionRepository>(
    repository: &R,
    definition: &ItemDefinition,
    instance: &ItemInstance,
) -> Result<Option<FragmentCapacity>, AppError> {
    if definition.bucket_hash != Some(SUBCLASS_BUCKET_HASH) {
        return Ok(None);
    }
    let hashes: Vec<_> = instance
        .sockets
        .iter()
        .filter_map(|socket| socket.plug_hash)
        .collect();
    let definitions = repository.lookup_definition_batch(&hashes)?;
    Ok(Some(FragmentCapacity {
        capacity: subclass_fragment_capacity(
            &definition.armor,
            (!instance.sockets.is_empty()).then_some(instance.sockets.as_slice()),
            |hash| {
                definitions
                    .iter()
                    .find(|definition| definition.hash.value() == i64::from(hash))
                    .map(|definition| &definition.armor)
            },
        ),
    }))
}
use destiny_core::domain::ItemSlot;
use destiny_core::{armor_catalog::*, armor_model::*, *};

/// Nameless `armor_stats` plugs (absent from `definitions`: blank name) that the
/// instance has inserted, resolved through the stat-plug store.
pub(crate) fn resolve_stat_plugs<R: ArmorCatalogRepository>(
    repository: &R,
    instance: &ItemInstance,
    definitions: &[ItemDefinition],
) -> Result<Vec<StatPlugDefinition>, AppError> {
    let missing: Vec<u32> = instance
        .sockets
        .iter()
        .filter_map(|socket| socket.plug_hash)
        .filter(|hash| {
            !definitions
                .iter()
                .any(|definition| definition.hash == *hash)
        })
        .filter_map(|hash| u32::try_from(hash.value()).ok())
        .collect();
    if missing.is_empty() {
        return Ok(Vec::new());
    }
    Ok(repository
        .armor_catalog_batch(ArmorCatalogKind::StatPlug, &missing)?
        .into_iter()
        .filter_map(|record| match record {
            ArmorCatalogRecord::StatPlug(plug) => Some(plug),
            _ => None,
        })
        .collect())
}

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
    // Blank-name Armor 3.0 stat-roll plugs are not in `definitions`; resolve them
    // into the same `ArmorDefinition` shape without inventing a name.
    let stat_plugs: Vec<_> = resolve_stat_plugs(repository, instance, &definitions)?
        .into_iter()
        .map(|plug| (plug.hash, plug.to_armor_definition()))
        .collect();
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
                    .map(|definition| &definition.armor)
                    .or_else(|| {
                        stat_plugs
                            .iter()
                            .find(|(plug_hash, _)| i64::from(*plug_hash) == hash.value())
                            .map(|(_, armor)| armor)
                    }),
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
    let armor_set = definition
        .armor
        .equipable_item_set_hash
        .filter(|hash| *hash != 0)
        .map(|hash| -> Result<ArmorSet, AppError> {
            let catalog = repository.lookup_armor_catalog(ArmorCatalogKind::EquipableSet, hash)?;
            let (name, perks) = match catalog {
                Some(ArmorCatalogRecord::EquipableSet(set)) => {
                    (Some(set.name), Some(set.set_perks))
                }
                _ => (None, None),
            };
            Ok(ArmorSet { hash, name, perks })
        })
        .transpose()?;
    Ok(Some(ArmorModel {
        tuning: armor_tuning(&instance.sockets, |hash| {
            definitions
                .iter()
                .find(|definition| definition.hash.value() == i64::from(hash))
                .map(|definition| &definition.armor)
        }),
        armor_set,
        stats,
        masterwork_tier,
        gear_tier: instance.gear_tier,
        // DIM inventory/store/d2-item-factory.ts:586 consumer-only fallback.
        effective_gear_tier: instance.gear_tier.unwrap_or(0),
        energy: instance.energy.clone(),
    }))
}
