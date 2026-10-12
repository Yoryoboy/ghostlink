//! Request-local evidence for build fashion (BE-007b3): the cosmetic sockets' types and
//! plug sets, the fashion plugs and their insertion costs, and the ornament unlocks that
//! plug sets do not report. Plug-set members are never loaded as definitions: DIM's
//! `plugFitsIntoSocket` only needs membership.
use destiny_core::{
    armor_catalog::*,
    build::BuildDefinition,
    build_capture::empty_plug,
    build_fashion_apply::{ARMOR_COSMETICS_CATEGORY, exotic_ornament_unlocks},
    mod_assignment::ARMOR_BUCKETS,
    ports::WorldDefinitionRepository,
    *,
};
use std::collections::BTreeSet;

#[derive(Debug, Default)]
pub(crate) struct FashionEvidence {
    pub catalogs: Vec<ArmorCatalogRecord>,
    pub ornament_unlocks: Vec<u32>,
}

/// `can_insert` lists the plugs the target's plug-set entries report as insertable
/// (universal ornaments use `canInsert` instead of `enabled`, `plugset-helpers.ts:26-36`).
pub(crate) fn fashion_evidence<R: WorldDefinitionRepository + ArmorCatalogRepository>(
    repository: &R,
    build: &BuildDefinition,
    items: &[ItemInstance],
    can_insert: &[u32],
    definitions: &mut Vec<ItemDefinition>,
) -> Result<FashionEvidence, AppError> {
    let requested: BTreeSet<u32> = build
        .parameters
        .as_ref()
        .and_then(|p| p.mods_by_bucket.as_ref())
        .into_iter()
        .flatten()
        .flat_map(|(_, hashes)| hashes.iter().map(|h| h.value() as u32))
        .collect();
    if requested.is_empty() {
        return Ok(FashionEvidence::default());
    }
    let mut wanted = requested.clone();
    let mut records = Vec::new();
    let mut keys = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for item in items {
        let Some(definition) = definitions.iter().find(|d| d.hash == item.item_hash) else {
            continue;
        };
        if !definition
            .bucket_hash
            .is_some_and(|b| ARMOR_BUCKETS.contains(&b.value()))
        {
            continue;
        }
        let cosmetic: BTreeSet<u32> = definition
            .armor
            .socket_categories
            .iter()
            .filter(|c| c.socket_category_hash == ARMOR_COSMETICS_CATEGORY)
            .flat_map(|c| c.socket_indexes.iter().copied())
            .collect();
        if seen.insert(definition.hash) {
            for entry in cosmetic
                .iter()
                .filter_map(|i| definition.armor.socket_entries.get(*i as usize))
            {
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
                            wanted.extend(
                                set.reusable_plug_items
                                    .iter()
                                    .map(|p| p.plug_item_hash)
                                    .filter(|h| empty_plug(i64::from(*h))),
                            );
                        }
                        records.push(record);
                    }
                }
                wanted.insert(entry.single_initial_item_hash);
                wanted.extend(entry.reusable_plug_items.iter().copied());
            }
        }
        for socket in item
            .sockets
            .iter()
            .filter(|s| cosmetic.contains(&u32::from(s.socket_index)))
        {
            wanted.extend(socket.plug_hash.map(|h| h.value() as u32));
            wanted.extend(
                socket
                    .reusable_plugs
                    .iter()
                    .flatten()
                    .map(|p| p.plug_hash.value() as u32)
                    .filter(|h| empty_plug(i64::from(*h))),
            );
        }
    }
    let missing: Vec<_> = wanted
        .iter()
        .filter(|hash| **hash != 0)
        .map(|hash| ItemHash::new(i64::from(*hash)))
        .filter(|hash| !definitions.iter().any(|d| d.hash == *hash))
        .collect();
    for chunk in missing.chunks(1024) {
        definitions.extend(repository.lookup_definition_batch(chunk)?);
    }
    let requirements: BTreeSet<_> = definitions
        .iter()
        .filter(|d| wanted.contains(&(d.hash.value() as u32)))
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
    let mut ornament_unlocks = exotic_ornament_unlocks(items, definitions);
    ornament_unlocks.extend(requested.iter().copied().filter(|hash| {
        can_insert.contains(hash)
            && definitions.iter().any(|d| {
                d.hash.value() == i64::from(*hash)
                    && d.armor.item_sub_type_code == Some(ITEM_SUB_TYPE_ORNAMENT)
            })
    }));
    ornament_unlocks.sort_unstable();
    ornament_unlocks.dedup();
    Ok(FashionEvidence {
        catalogs: records,
        ornament_unlocks,
    })
}

/// `DestinyItemSubType.Ornament` (bungie-api-ts).
const ITEM_SUB_TYPE_ORNAMENT: u8 = 21;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{InMemoryDefinitionRepository, synthetic_world};
    use destiny_core::ports::{DefinitionReplacementInput, ManifestDefinitions};

    fn def(hash: i64, bucket: Option<i64>) -> ItemDefinition {
        ItemDefinition {
            hash: ItemHash::new(hash),
            name: format!("Item {hash}"),
            bucket_hash: bucket.map(BucketHash::new),
            tier: Some("Legendary".into()),
            item_type: None,
            element: None,
            slot: None,
            class_type: None,
            plug_category: None,
            tags: vec![],
            version: DefinitionVersion::new("v1"),
            armor: Default::default(),
            non_transferrable: None,
            is_instance_item: Some(true),
            does_postmaster_pull_have_side_effects: None,
            max_stack_size: None,
            stack_unique_label: None,
        }
    }

    /// One helmet whose cosmetic socket (index 0) takes plug set 201: the default
    /// ornament (empty, 702981643), a requested ornament (10) and a costly shader (11).
    fn repository() -> (InMemoryDefinitionRepository, ItemInstance) {
        let mut helmet = def(1, Some(3448274439));
        helmet.armor.socket_entries.push(DefinitionSocketEntry {
            socket_type_hash: 99,
            reusable_plug_set_hash: Some(201),
            ..Default::default()
        });
        helmet
            .armor
            .socket_categories
            .push(DefinitionSocketCategory {
                socket_category_hash: 1926152773,
                socket_indexes: vec![0],
            });
        // Not a cosmetic socket: must not be loaded.
        helmet.armor.socket_entries.push(DefinitionSocketEntry {
            socket_type_hash: 98,
            reusable_plug_set_hash: Some(202),
            ..Default::default()
        });
        let plug = |hash, sub_type, requirement| {
            let mut d = def(hash, None);
            d.armor.plug_category_hash = Some(7);
            d.armor.item_sub_type_code = Some(sub_type);
            d.armor.insertion_material_requirement_hash = requirement;
            d
        };
        let mut world = synthetic_world(DefinitionVersion::new("v1"));
        let set = |hash, members: &[u32]| {
            ArmorCatalogRecord::PlugSet(PlugSetDefinition {
                hash,
                reusable_plug_items: members
                    .iter()
                    .map(|plug_item_hash| PlugSetItem {
                        plug_item_hash: *plug_item_hash,
                        currently_can_roll: true,
                    })
                    .collect(),
            })
        };
        let kind = |hash| {
            ArmorCatalogRecord::SocketType(SocketTypeDefinition {
                hash,
                socket_category_hash: 100,
                always_randomize_sockets: false,
                plug_whitelist: vec![],
            })
        };
        world.armor_catalogs = vec![
            kind(99),
            kind(98),
            set(201, &[702981643, 10, 11]),
            set(202, &[12]),
            ArmorCatalogRecord::MaterialRequirementSet(MaterialRequirementSetDefinition {
                hash: 301,
                materials: vec![],
            }),
        ];
        let mut repository = InMemoryDefinitionRepository::default();
        repository
            .publish_manifest(
                DefinitionReplacementInput {
                    definitions: ManifestDefinitions {
                        version: DefinitionVersion::new("v1"),
                        definitions: vec![
                            helmet,
                            plug(702981643, 21, None),
                            plug(10, 21, None),
                            plug(11, 20, Some(301)),
                            plug(12, 20, None),
                        ],
                    },
                    checked_at: Timestamp::new(0),
                    source: SourceMarker::Manifest,
                },
                world,
            )
            .unwrap();
        let item = ItemInstance {
            instance_id: ItemInstanceId::new("a"),
            item_hash: ItemHash::new(1),
            location: ItemLocation::Vault,
            equipped: false,
            locked: false,
            quantity: 1,
            power: None,
            transfer_status: None,
            energy: None,
            gear_tier: None,
            sockets: vec![],
            available_plugs: vec![],
            perks: vec![],
            plugs: vec![],
            stats: vec![],
        };
        (repository, item)
    }
    fn build(hashes: &[i64]) -> BuildDefinition {
        BuildDefinition {
            parameters: Some(destiny_core::build::BuildParameters {
                mods_by_bucket: Some(
                    [(
                        BucketHash::new(3448274439),
                        hashes.iter().map(|h| ItemHash::new(*h)).collect(),
                    )]
                    .into(),
                ),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn loads_only_the_cosmetic_sockets_requested_plugs_and_their_costs() {
        let (repository, item) = repository();
        let mut definitions = vec![repository_definition(&repository, 1)];
        let evidence = fashion_evidence(
            &repository,
            &build(&[10, 11]),
            std::slice::from_ref(&item),
            &[],
            &mut definitions,
        )
        .unwrap();
        let mut hashes: Vec<_> = definitions.iter().map(|d| d.hash.value()).collect();
        hashes.sort_unstable();
        // The requested plugs and the socket's default ornament, not the unrelated set.
        assert_eq!(hashes, [1, 10, 11, 702981643]);
        let kinds: Vec<_> = evidence
            .catalogs
            .iter()
            .map(|r| match r {
                ArmorCatalogRecord::SocketType(t) => ("type", t.hash),
                ArmorCatalogRecord::PlugSet(s) => ("set", s.hash),
                ArmorCatalogRecord::MaterialRequirementSet(s) => ("req", s.hash),
                _ => ("other", 0),
            })
            .collect();
        assert_eq!(kinds, [("type", 99), ("set", 201), ("req", 301)]);
    }

    #[test]
    fn ornament_unlocks_are_requested_insertable_ornaments_only() {
        let (repository, item) = repository();
        let mut definitions = vec![repository_definition(&repository, 1)];
        let evidence = fashion_evidence(
            &repository,
            &build(&[10, 11]),
            std::slice::from_ref(&item),
            &[10, 11, 12],
            &mut definitions,
        )
        .unwrap();
        // 10 is an insertable ornament; 11 is a shader and 12 was not requested.
        assert_eq!(evidence.ornament_unlocks, [10]);
    }

    #[test]
    fn builds_without_fashion_load_nothing() {
        let (repository, item) = repository();
        let mut definitions = vec![repository_definition(&repository, 1)];
        let evidence = fashion_evidence(
            &repository,
            &BuildDefinition::default(),
            std::slice::from_ref(&item),
            &[10],
            &mut definitions,
        )
        .unwrap();
        assert!(evidence.catalogs.is_empty() && evidence.ornament_unlocks.is_empty());
        assert_eq!(definitions.len(), 1);
    }

    fn repository_definition(
        repository: &InMemoryDefinitionRepository,
        hash: i64,
    ) -> ItemDefinition {
        match repository.lookup_definition(ItemHash::new(hash)).unwrap() {
            Resolution::Unique(d) => d,
            _ => panic!("definition"),
        }
    }
}
