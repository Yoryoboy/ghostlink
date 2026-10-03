//! Live-only, provider-neutral saved loadouts.
use crate::{AppError, CharacterId, DestinyMembership, ErrorCategory, ItemHash, ItemInstanceId};
use std::collections::HashSet;

pub const MAX_LOADOUT_CHARACTERS: usize = 8;
pub const MAX_CHARACTER_LOADOUTS: usize = 12;
pub const MAX_LOADOUT_ITEMS: usize = 16;
pub const MAX_LOADOUT_PLUGS: usize = 32;
pub const UNSET_LOADOUT_PLUG: u32 = 2166136261;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutSnapshot {
    pub characters: Vec<CharacterLoadouts>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterLoadouts {
    pub character_id: CharacterId,
    pub loadouts: Vec<Loadout>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Loadout {
    /// Original zero-based position, including omitted unused slots.
    pub slot_index: u8,
    pub name_hash: u32,
    pub color_hash: u32,
    pub icon_hash: u32,
    pub items: Vec<LoadoutItem>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutItem {
    pub instance_id: ItemInstanceId,
    pub plugs: Vec<LoadoutPlug>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutPlug {
    /// Original zero-based socket position, including omitted unset plugs.
    pub socket_index: u8,
    pub plug_hash: ItemHash,
}
/// Matches the bounded canonical decimal identifiers used by profile reads.
pub fn is_canonical_loadout_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().all(|b| b.is_ascii_digit())
        && (id == "0" || !id.starts_with('0'))
}
impl LoadoutSnapshot {
    /// Revalidates bounds, identifiers and retained positions for constructed data.
    pub fn validate(&self) -> Result<(), AppError> {
        let invalid = || AppError::new(ErrorCategory::ProviderValidation);
        if self.characters.len() > MAX_LOADOUT_CHARACTERS {
            return Err(invalid());
        }
        let mut characters = HashSet::new();
        for character in &self.characters {
            if !is_canonical_loadout_id(character.character_id.as_str())
                || !characters.insert(&character.character_id)
                || character.loadouts.len() > MAX_CHARACTER_LOADOUTS
            {
                return Err(invalid());
            }
            let mut slots = HashSet::new();
            for loadout in &character.loadouts {
                if usize::from(loadout.slot_index) >= MAX_CHARACTER_LOADOUTS
                    || !slots.insert(loadout.slot_index)
                    || loadout.items.is_empty()
                    || loadout.items.len() > MAX_LOADOUT_ITEMS
                {
                    return Err(invalid());
                }
                for item in &loadout.items {
                    if !is_canonical_loadout_id(item.instance_id.as_str())
                        || item.instance_id.as_str() == "0"
                        || item.plugs.len() > MAX_LOADOUT_PLUGS
                    {
                        return Err(invalid());
                    }
                    let mut sockets = HashSet::new();
                    for plug in &item.plugs {
                        if usize::from(plug.socket_index) >= MAX_LOADOUT_PLUGS
                            || !sockets.insert(plug.socket_index)
                            || u32::try_from(plug.plug_hash.value()).is_err()
                            || plug.plug_hash.value() == i64::from(UNSET_LOADOUT_PLUG)
                        {
                            return Err(invalid());
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
/// Separate live read port; no persistence or inventory gateway changes.
pub trait LoadoutGateway {
    fn load_loadouts(
        &mut self,
        membership: &DestinyMembership,
    ) -> Result<LoadoutSnapshot, AppError>;
}
