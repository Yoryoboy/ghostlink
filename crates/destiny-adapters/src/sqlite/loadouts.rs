//! A normalized JSON projection, independent of mutable inventory rows.
use super::storage_failure;
use destiny_core::*;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct SavedCharacter {
    id: String,
    loadouts: Vec<SavedLoadout>,
}
#[derive(Serialize, Deserialize)]
struct SavedLoadout {
    slot: u8,
    name: u32,
    color: u32,
    icon: u32,
    items: Vec<SavedItem>,
}
#[derive(Serialize, Deserialize)]
struct SavedItem {
    id: String,
    plugs: Vec<(u8, i64)>,
}

pub(super) fn encode(rows: Vec<CharacterLoadouts>) -> Result<String, AppError> {
    LoadoutSnapshot {
        characters: rows.clone(),
    }
    .validate()?;
    let wire: Vec<_> = rows
        .into_iter()
        .map(|c| SavedCharacter {
            id: c.character_id.as_str().to_owned(),
            loadouts: c
                .loadouts
                .into_iter()
                .map(|l| SavedLoadout {
                    slot: l.slot_index,
                    name: l.name_hash,
                    color: l.color_hash,
                    icon: l.icon_hash,
                    items: l
                        .items
                        .into_iter()
                        .map(|i| SavedItem {
                            id: i.instance_id.as_str().to_owned(),
                            plugs: i
                                .plugs
                                .into_iter()
                                .map(|p| (p.socket_index, p.plug_hash.value()))
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();
    serde_json::to_string(&wire).map_err(|_| storage_failure())
}

pub(super) fn read(connection: &Connection) -> Result<Option<Vec<CharacterLoadouts>>, AppError> {
    let raw: Option<String> = connection
        .query_row(
            "SELECT normalized_loadouts FROM saved_loadouts WHERE singleton = 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| storage_failure())?;
    raw.map(|raw| {
        let wire: Vec<SavedCharacter> =
            serde_json::from_str(&raw).map_err(|_| storage_failure())?;
        let characters = wire
            .into_iter()
            .map(|c| CharacterLoadouts {
                character_id: CharacterId::new(c.id),
                loadouts: c
                    .loadouts
                    .into_iter()
                    .map(|l| Loadout {
                        slot_index: l.slot,
                        name_hash: l.name,
                        color_hash: l.color,
                        icon_hash: l.icon,
                        items: l
                            .items
                            .into_iter()
                            .map(|i| LoadoutItem {
                                instance_id: ItemInstanceId::new(i.id),
                                plugs: i
                                    .plugs
                                    .into_iter()
                                    .map(|(socket_index, hash)| LoadoutPlug {
                                        socket_index,
                                        plug_hash: ItemHash::new(hash),
                                    })
                                    .collect(),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        let snapshot = LoadoutSnapshot { characters };
        snapshot.validate().map_err(|_| storage_failure())?;
        Ok(snapshot.characters)
    })
    .transpose()
}
