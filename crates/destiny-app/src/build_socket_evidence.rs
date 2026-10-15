//! Request-local catalog assembly for socket-override planning (BE-007b).
use destiny_core::{
    armor_catalog::*, build_analysis::ResolvedBuildItem, ports::WorldDefinitionRepository, *,
};
use std::collections::BTreeSet;

/// Socket types, plug sets and material requirement sets for every selected item that
/// carries overrides, plus the definitions of the plugs those sockets can hold (empty
/// and current plugs decide `canInsertForFree`). Definitions are appended once.
pub(crate) fn socket_catalogs<R: WorldDefinitionRepository + ArmorCatalogRepository>(
    repository: &R,
    selected: &[ResolvedBuildItem],
    definitions: &mut Vec<ItemDefinition>,
) -> Result<Vec<ArmorCatalogRecord>, AppError> {
    let mut records = Vec::new();
    let mut keys = BTreeSet::new();
    let mut plugs = BTreeSet::new();
    for item in selected
        .iter()
        .filter(|s| !s.spec.socket_overrides.is_empty())
        .filter_map(|s| s.inventory.as_ref())
    {
        let Some(definition) = definitions.iter().find(|d| d.hash == item.item_hash) else {
            continue;
        };
        for entry in &definition.armor.socket_entries {
            let set = entry
                .reusable_plug_set_hash
                .or(entry.randomized_plug_set_hash);
            for (kind, hash) in [
                (ArmorCatalogKind::SocketType, Some(entry.socket_type_hash)),
                (ArmorCatalogKind::PlugSet, set),
            ] {
                if let Some(hash) = hash
                    && keys.insert((kind.id(), hash))
                    && let Some(record) = repository.lookup_armor_catalog(kind, hash)?
                {
                    if let ArmorCatalogRecord::PlugSet(set) = &record {
                        plugs.extend(set.reusable_plug_items.iter().map(|p| p.plug_item_hash));
                    }
                    records.push(record);
                }
            }
            plugs.extend(entry.reusable_plug_items.iter().copied());
            plugs.insert(entry.single_initial_item_hash);
        }
        plugs.extend(
            item.sockets
                .iter()
                .flat_map(|s| s.reusable_plugs.iter().flatten())
                .map(|p| p.plug_hash.value() as u32),
        );
    }
    let missing: Vec<_> = plugs
        .into_iter()
        .filter(|hash| *hash != 0)
        .map(|hash| ItemHash::new(i64::from(hash)))
        .filter(|hash| !definitions.iter().any(|d| d.hash == *hash))
        .collect();
    for chunk in missing.chunks(1024) {
        definitions.extend(repository.lookup_definition_batch(chunk)?);
    }
    let requirements: BTreeSet<_> = definitions
        .iter()
        .filter_map(|d| d.armor.insertion_material_requirement_hash)
        .filter(|hash| *hash != 0)
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
