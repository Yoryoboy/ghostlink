//! GetProfile component 206 only; unknown provider fields are ignored.
use destiny_core::*;
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use std::fmt;

pub const LOADOUT_RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
fn invalid() -> AppError {
    AppError::new(ErrorCategory::ProviderValidation)
}
/// Canonical identifiers only, before any credential is attached.
pub fn loadout_endpoint(membership: &DestinyMembership) -> Result<String, AppError> {
    let url = format!(
        "https://www.bungie.net/Platform/Destiny2/{}/Profile/{}/?components=206",
        membership.membership_type().value(),
        membership.membership_id().as_str()
    );
    if !is_allowed_loadout_url(&url) {
        return Err(invalid());
    }
    Ok(url)
}
/// Accepts only the canonical origin, decimal identifiers and exact query.
pub fn is_allowed_loadout_url(url: &str) -> bool {
    let Some(path) = url.strip_prefix("https://www.bungie.net/Platform/Destiny2/") else {
        return false;
    };
    let parts: Vec<_> = path.split('/').collect();
    parts.len() == 4
        && is_canonical_loadout_id(parts[0])
        && parts[0].parse::<u32>().is_ok_and(|v| v != 0 && v != 254)
        && parts[1] == "Profile"
        && is_canonical_loadout_id(parts[2])
        && parts[3] == "?components=206"
}
#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "ErrorCode")]
    code: u32,
    #[serde(rename = "Response")]
    response: Response,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    character_loadouts: Option<Component>,
}
#[derive(Deserialize)]
struct Component {
    data: Option<Characters>,
}
// Preserve map entries so duplicate JSON keys cannot silently overwrite a character.
struct Characters(Vec<(String, WireCharacter)>);
impl<'de> Deserialize<'de> for Characters {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CharacterVisitor;
        impl<'de> Visitor<'de> for CharacterVisitor {
            type Value = Characters;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded character loadouts map")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Characters, M::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry::<String, WireCharacter>()? {
                    if entries.len() >= MAX_LOADOUT_CHARACTERS {
                        return Err(serde::de::Error::custom("too many characters"));
                    }
                    entries.push(entry);
                }
                Ok(Characters(entries))
            }
        }
        deserializer.deserialize_map(CharacterVisitor)
    }
}
#[derive(Deserialize)]
struct WireCharacter {
    loadouts: Vec<WireLoadout>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireLoadout {
    name_hash: u32,
    color_hash: u32,
    icon_hash: u32,
    items: Vec<WireItem>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireItem {
    item_instance_id: String,
    plug_item_hashes: Vec<u32>,
}
/// Decodes saved loadouts; absent/private data is empty. Bounds apply before filtering.
pub fn decode_loadouts(bytes: &[u8]) -> Result<LoadoutSnapshot, AppError> {
    if bytes.len() > LOADOUT_RESPONSE_LIMIT {
        return Err(invalid());
    }
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if envelope.code != 1 {
        return Err(invalid());
    }
    let mut snapshot = LoadoutSnapshot {
        characters: Vec::new(),
    };
    if let Some(Characters(characters)) = envelope.response.character_loadouts.and_then(|c| c.data)
    {
        for (id, character) in characters {
            if character.loadouts.len() > MAX_CHARACTER_LOADOUTS {
                return Err(invalid());
            }
            let mut loadouts = Vec::new();
            for (slot_index, loadout) in character.loadouts.into_iter().enumerate() {
                if loadout.items.len() > MAX_LOADOUT_ITEMS {
                    return Err(invalid());
                }
                let mut items = Vec::new();
                for item in loadout.items {
                    if !is_canonical_loadout_id(&item.item_instance_id)
                        || item.plug_item_hashes.len() > MAX_LOADOUT_PLUGS
                    {
                        return Err(invalid());
                    }
                    if item.item_instance_id == "0" {
                        continue;
                    }
                    let plugs = item
                        .plug_item_hashes
                        .into_iter()
                        .enumerate()
                        .filter(|(_, hash)| *hash != UNSET_LOADOUT_PLUG)
                        .map(|(socket_index, hash)| LoadoutPlug {
                            socket_index: socket_index as u8,
                            plug_hash: ItemHash::new(i64::from(hash)),
                        })
                        .collect();
                    items.push(LoadoutItem {
                        instance_id: ItemInstanceId::new(item.item_instance_id),
                        plugs,
                    });
                }
                if !items.is_empty() {
                    loadouts.push(Loadout {
                        slot_index: slot_index as u8,
                        name_hash: loadout.name_hash,
                        color_hash: loadout.color_hash,
                        icon_hash: loadout.icon_hash,
                        items,
                    });
                }
            }
            snapshot.characters.push(CharacterLoadouts {
                character_id: CharacterId::new(id),
                loadouts,
            });
        }
    }
    snapshot.validate()?;
    Ok(snapshot)
}
