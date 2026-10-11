//! Pure mapping from a Bungie `Destiny2.GetProfile` response to the
//! provider-neutral [`InventorySnapshotInput`] in `destiny-core`.
//!
//! This module performs no I/O, no scheduling, and no provider calls. It
//! validates one complete profile response and normalizes the characters and
//! instanced items and non-instanced inventory stacks it contains. The application
//! profile service still decides whether to publish the candidate and owns revision, freshness, and
//! provenance; this decoder only attaches the caller-supplied membership,
//! definition version, timestamp, and the fixed [`SourceMarker::Profile`].
//!
//! Input is handled as `serde_json::Value` only; the adapter never
//! deserializes into provider structs. The candidate is strict and complete:
//! every required component wrapper must be present with `data`, no component
//! may be `disabled`, and a privacy value outside `0` (None), `1` (Public), and
//! `2` (Private) is rejected. Bungie returns owner-only components as privacy
//! `2` with `data` for the authorized owner, so a private component is accepted
//! exactly when its `data` is present. Characters, profile inventories, character inventories, character
//! equipment, item instances, and item perks must all be internally
//! consistent. Optional currency coverage is qualified separately: genuinely
//! unavailable data permits ordinary gear reads, but malformed present currency
//! records refuse the whole candidate. No missing wrapper means balance zero.
//!
//! Every failure is a sanitized `ErrorCategory::ProviderValidation` error.
//! Neither raw provider JSON nor any provider message, identifier, or path can
//! escape through the failure path, because the only payload an error carries
//! is its category.
//!
//! Normalization rules:
//!
//! - Missing and `null` instance identifiers produce validated stack observations
//!   with positive int32 quantities and mandatory runtime buckets. They never
//!   join per-instance components or receive fabricated identifiers. Duplicate
//!   rows remain distinct. Profile stacks retain their ProfileInventory source.
//! - Literal `"0"` remains an instance identifier under the existing canonical
//!   decimal parser; no zero-as-stack wire convention is established here.
//! - Character location Postmaster (`4`) maps to the validated map-key owner
//!   for gear and stacks. Equipped Postmaster rows are rejected. Profile-origin
//!   location `4` fails closed because that component supplies no character owner.
//! - Profile gear location Vault (`2`) maps to [`ItemLocation::Vault`]; other
//!   profile locations are preserved as `Other("bungie-profile-location:<n>")`.
//!   Normal character inventory/equipment retains its owner and equipped status.
//!   Absent character location retains the existing source-based classification;
//!   supplied location values must be uint32.
//! - Component 103 retains separate ProfileCurrencies provenance, including
//!   zero balances and duplicates; overlapping inventory rows are never summed.
//!   Missing legacy wrappers mean NotObserved; null, disabled or missing data
//!   mean Unavailable. Usable private data is accepted. Malformed data fails closed.
//! - Character runtime Lost Items buckets also identify owned Postmaster rows.
//!   Official Manifest provenance is documented on LOST_ITEMS_BUCKET_HASH;
//!   decoder fixtures are synthetic and do not establish live-account coverage.
//! - State bit `1` marks an item locked. An item's equipment status must agree
//!   with the provider `isEquipped` flag, so a vault item or a character
//!   inventory item can never be equipped while an equipment item must be.
//! - `primaryStat.value` maps to an optional `u16` power, where an integer
//!   outside that range is an unknown power, and `light` maps to an optional
//!   `u16`.
//! - Perk summaries preserve provider order and duplicate hashes. Per the
//!   official `Destiny.Perks.DestinyPerkReference` contract, `perkHash` is a
//!   `uint32` and `isActive` and `visible` are booleans; both flags are kept
//!   independently of each other.
//! - Characters and items are emitted in ascending numeric decimal-identifier
//!   order. Accepted identifiers are canonical, so every identifier is either
//!   the literal `0` or has a nonzero leading digit.

use destiny_core::{
    AppError, BucketHash, Character, CharacterId, DefinitionVersion, DestinyMembership,
    ErrorCategory, InstancedBucketObservation, InventorySnapshotInput, ItemHash, ItemInstance,
    ItemInstanceId, ItemLocation, PerkHash, PerkSummary, PlugSummary, ProfileAcquisition,
    SourceMarker, StackContainer, StackObservation, StatSummary, Timestamp,
};
use serde_json::{Map, Value};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashSet};

/// Unlike `Value`, this reader rejects repeated object keys before they can be
/// overwritten (including repeated stat hashes).
struct UniqueValue(Value);

impl<'de> serde::Deserialize<'de> for UniqueValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut access: M,
            ) -> Result<Self::Value, M::Error> {
                let mut map = Map::new();
                while let Some((key, UniqueValue(value))) =
                    access.next_entry::<String, UniqueValue>()?
                {
                    if map.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate key"));
                    }
                }
                Ok(UniqueValue(Value::Object(map)))
            }
            fn visit_seq<S: serde::de::SeqAccess<'de>>(
                self,
                mut access: S,
            ) -> Result<Self::Value, S::Error> {
                let mut values = Vec::new();
                while let Some(UniqueValue(value)) = access.next_element()? {
                    values.push(value);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Bool(value)))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::from(value)))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::from(value)))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::from(value)))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::from(value)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

/// Provider privacy values that are accepted when present.
const PRIVACY_NONE: u64 = 0;
const PRIVACY_PUBLIC: u64 = 1;
const PRIVACY_PRIVATE: u64 = 2;

/// The longest decimal identifier the durable profile projection accepts.
const MAX_IDENTIFIER_DIGITS: usize = 128;

/// Bound all sockets, including empty and disabled entries, before allocation.
const MAX_SOCKETS_PER_ITEM: usize = destiny_core::MAX_ITEM_SOCKETS;

/// Official provider profile locations (`Destiny.DestinyItemLocation`).
///
/// `Unknown = 0`, `Inventory = 1`, `Vendor = 3`, and any future value above
/// `Postmaster` are preserved verbatim as `Other`; `Vault = 2` maps to
/// [`ItemLocation::Vault`]. `Postmaster = 4` requires a validated character owner;
/// profile-origin Postmaster cannot be represented safely and is rejected.
const LOCATION_VAULT: u32 = 2;
const LOCATION_POSTMASTER: u32 = 4;

/// Actual Lost Items container, confirmed by the official public English Manifest
/// bucket definitions (not an item's normal equipment definition bucket):
/// <https://www.bungie.net/common/destiny2_content/json/en/DestinyInventoryBucketDefinition-a8ba855a-93c7-4014-8f2e-d92357fdfb42.json>.
/// Definition: location 4, itemCount 21, fifo true, hasTransferDestination false.
/// Tests using this value are synthetic, not live-account coverage evidence.
const LOST_ITEMS_BUCKET_HASH: u32 = 215_593_132;

fn provider_validation() -> AppError {
    AppError::new(ErrorCategory::ProviderValidation)
}

/// Decodes one complete Bungie profile response into a normalized candidate.
///
/// The caller owns the resolved [`DestinyMembership`], the current
/// [`DefinitionVersion`], and the acquisition [`Timestamp`]; the decoder
/// attaches them unchanged and fixes the source to [`SourceMarker::Profile`].
/// Missing or disabled required components and malformed present input are
/// sanitized [`ErrorCategory::ProviderValidation`] failures. Optional currencies
/// retain explicit coverage; absent legacy wrappers are NotObserved, while
/// unusable wrappers are Unavailable. Usable private data is accepted.
///
/// [`ErrorCategory::ProviderValidation`]: destiny_core::ErrorCategory::ProviderValidation
pub fn decode_profile(
    bytes: &[u8],
    membership: &DestinyMembership,
    definition_version: DefinitionVersion,
    checked_at: Timestamp,
) -> Result<InventorySnapshotInput, AppError> {
    decode_profile_with_observations(bytes, membership, definition_version, checked_at)
        .map(|pair| pair.into_parts().0)
}

/// Decodes the same candidate with complete actual runtime bucket observations.
/// Omitted instanced buckets make the entire observation set unavailable without
/// changing the input. Present malformed buckets (including null) fail closed.
/// This structural association confers no freshness or action authority.
pub fn decode_profile_with_observations(
    bytes: &[u8],
    membership: &DestinyMembership,
    definition_version: DefinitionVersion,
    checked_at: Timestamp,
) -> Result<ProfileAcquisition, AppError> {
    let UniqueValue(value) = serde_json::from_slice(bytes).map_err(|_| provider_validation())?;
    let root = value.as_object().ok_or_else(provider_validation)?;

    crate::bungie_errors::read_error(root)?;

    let response = root
        .get("Response")
        .and_then(Value::as_object)
        .ok_or_else(provider_validation)?;

    // Minted time is ordering evidence only. Like DIM, an absent or unusable value
    // never fails the sync; it is treated as unknown, so the profile publishes.
    let minted = response
        .get("responseMintedTimestamp")
        .and_then(Value::as_str)
        .and_then(|raw| {
            // Chrono truncates fractions beyond nanoseconds. Treat the value as
            // unknown rather than equate two distinct upstream instants.
            if let Some((_, fraction)) = raw.split_once('.')
                && fraction.bytes().take_while(u8::is_ascii_digit).count() > 9
            {
                return None;
            }
            let date = chrono::DateTime::parse_from_rfc3339(raw).ok()?;
            destiny_core::ProfileMintedAt::new(date.timestamp(), date.timestamp_subsec_nanos()).ok()
        });

    let characters = decode_characters(required_component(response, "characters")?)?;
    let character_ids: BTreeSet<String> = characters
        .iter()
        .map(|character| character.id.as_str().to_owned())
        .collect();

    let mut raw_items = decode_profile_items(required_component(response, "profileInventory")?)?;
    raw_items.extend(decode_character_items(
        required_component(response, "characterInventories")?,
        &character_ids,
        false,
    )?);
    raw_items.extend(decode_character_items(
        required_component(response, "characterEquipment")?,
        &character_ids,
        true,
    )?);

    let item_components = response
        .get("itemComponents")
        .and_then(Value::as_object)
        .ok_or_else(provider_validation)?;
    let instances = component_data(
        item_components
            .get("instances")
            .ok_or_else(provider_validation)?,
    )?;
    let perks = component_data(
        item_components
            .get("perks")
            .ok_or_else(provider_validation)?,
    )?;

    let sockets = component_data(
        item_components
            .get("sockets")
            .ok_or_else(provider_validation)?,
    )?;

    let stats = component_data(
        item_components
            .get("stats")
            .ok_or_else(provider_validation)?,
    )?;

    let reusable = component_data(
        item_components
            .get("reusablePlugs")
            .ok_or_else(provider_validation)?,
    )?;

    let mut seen = HashSet::with_capacity(raw_items.len());
    let mut items = Vec::with_capacity(raw_items.len());
    let mut stacks = Vec::new();
    let mut observations = Some(Vec::with_capacity(raw_items.len()));
    for row in raw_items {
        let raw = match row {
            RawItem::Stack(stack) => {
                stacks.push(stack);
                continue;
            }
            RawItem::Instanced(raw) => raw,
        };
        if !seen.insert(raw.instance_id.clone()) {
            return Err(provider_validation());
        }

        let instance = instances
            .get(&raw.instance_id)
            .and_then(Value::as_object)
            .ok_or_else(provider_validation)?;
        let is_equipped = instance
            .get("isEquipped")
            .and_then(Value::as_bool)
            .ok_or_else(provider_validation)?;
        if is_equipped != raw.equipped {
            return Err(provider_validation());
        }
        let power = decode_power(instance)?;
        let item_perks = decode_perks(perks, &raw.instance_id)?;
        let indexed_sockets = decode_sockets(sockets, &raw.instance_id)?;
        let plugs = indexed_sockets
            .iter()
            .filter_map(|socket| {
                socket
                    .plug_hash
                    .filter(|_| socket.enabled)
                    .map(|plug_hash| PlugSummary {
                        plug_hash,
                        enabled: socket.enabled,
                    })
            })
            .collect();
        let stats = decode_stats(stats, &raw.instance_id)?;

        match (raw.runtime_bucket, &mut observations) {
            (Some(bucket), Some(rows)) => rows.push(InstancedBucketObservation::new(
                ItemInstanceId::new(raw.instance_id.clone()),
                BucketHash::new(i64::from(bucket)),
                raw.location.clone(),
            )?),
            (None, _) => observations = None,
            _ => {}
        }
        items.push(ItemInstance {
            energy: decode_energy(instance)?,
            gear_tier: optional_u32(instance, "gearTier")?,
            sockets: indexed_sockets,
            transfer_status: raw.transfer_status,
            available_plugs: decode_available_plugs(reusable, &raw.instance_id)?,
            instance_id: ItemInstanceId::new(raw.instance_id),
            item_hash: raw.item_hash,
            location: raw.location,
            quantity: raw.quantity,
            equipped: is_equipped,
            locked: raw.locked,
            power,
            perks: item_perks,
            plugs,
            stats,
        });
    }

    let mut characters = characters;
    characters.sort_by(|left, right| decimal_id_order(left.id.as_str(), right.id.as_str()));
    items.sort_by(|left, right| {
        decimal_id_order(left.instance_id.as_str(), right.instance_id.as_str())
    });

    let (profile_currencies_coverage, currencies) = decode_profile_currencies(response)?;
    stacks.extend(currencies);
    let snapshot = InventorySnapshotInput {
        profile_currencies_coverage,
        stacks,
        plug_set_entries: decode_plug_sets(response, &character_ids)?,
        membership_id: membership.membership_id().clone(),
        membership_type: membership.membership_type(),
        definition_version,
        checked_at,
        source: SourceMarker::Profile,
        characters,
        items,
    };

    // Optional 206 follows minted-time leniency: unavailable or malformed data
    // must not prevent publishing an otherwise valid inventory profile.
    let loadouts = response
        .get("characterLoadouts")
        .filter(|component| component.get("disabled") != Some(&Value::Bool(true)))
        .filter(|component| component.get("data").is_some_and(|data| !data.is_null()))
        // Apply the 206 decoder's response bound to 206, not the larger profile.
        .and_then(|component| {
            serde_json::to_vec(&serde_json::json!({
                "ErrorCode": 1, "Response": {"characterLoadouts": component}
            }))
            .ok()
        })
        .and_then(|bytes| crate::loadout::decode_loadouts(&bytes).ok())
        .map(|snapshot| snapshot.characters);
    ProfileAcquisition::new(snapshot, observations)
        .map(|pair| pair.with_minted_at(minted).with_saved_loadouts(loadouts))
}

/// Optional owner-only components can be absent or withheld for privacy.
fn optional_plug_set_data<'a>(
    response: &'a Map<String, Value>,
    name: &str,
) -> Result<Option<&'a Map<String, Value>>, AppError> {
    let Some(wrapper) = response.get(name).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let object = wrapper.as_object().ok_or_else(provider_validation)?;
    if object.get("disabled") == Some(&Value::Bool(true)) {
        return Ok(None);
    }
    if object.get("data").is_none_or(Value::is_null) {
        if object.get("privacy").and_then(Value::as_u64) == Some(PRIVACY_PRIVATE) {
            return Ok(None);
        }
        return Err(provider_validation());
    }
    component_data(wrapper).map(Some)
}

/// Bound all sets across both components before merging identical scoped hashes.
fn decode_plug_sets(
    response: &Map<String, Value>,
    character_ids: &BTreeSet<String>,
) -> Result<Vec<destiny_core::PlugSetEntry>, AppError> {
    use destiny_core::{PlugSetEntry, PlugSetScope};
    let mut entries = std::collections::BTreeMap::new();
    let mut set_count = 0;
    let mut merge = |scope: PlugSetScope, data: &Map<String, Value>| -> Result<(), AppError> {
        let sets = data
            .get("plugs")
            .and_then(Value::as_object)
            .ok_or_else(provider_validation)?;
        set_count += sets.len();
        if set_count > 4096 {
            return Err(provider_validation());
        }
        for (key, value) in sets {
            validate_decimal_id(key)?;
            key.parse::<u32>().map_err(|_| provider_validation())?;
            let plugs = value.as_array().ok_or_else(provider_validation)?;
            if plugs.len() > 512 {
                return Err(provider_validation());
            }
            for plug in plugs {
                let hash = decode_item_hash(plug.get("plugItemHash"))?;
                let can_insert = plug
                    .get("canInsert")
                    .and_then(Value::as_bool)
                    .ok_or_else(provider_validation)?;
                let enabled = plug
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(provider_validation)?;
                let entry = entries
                    .entry((scope.clone(), hash))
                    .or_insert(PlugSetEntry {
                        scope: scope.clone(),
                        plug_hash: hash,
                        can_insert: false,
                        enabled: false,
                    });
                entry.can_insert |= can_insert;
                entry.enabled |= enabled;
            }
        }
        Ok(())
    };
    if let Some(data) = optional_plug_set_data(response, "profilePlugSets")? {
        merge(PlugSetScope::Profile, data)?;
    }
    if let Some(data) = optional_plug_set_data(response, "characterPlugSets")? {
        if data.len() > character_ids.len() {
            return Err(provider_validation());
        }
        for (id, value) in data {
            validate_decimal_id(id)?;
            if !character_ids.contains(id) {
                return Err(provider_validation());
            }
            merge(
                PlugSetScope::Character(CharacterId::new(id)),
                value.as_object().ok_or_else(provider_validation)?,
            )?;
        }
    }
    Ok(entries.into_values().collect())
}

/// Selectable plugs are bounded before filtering and sorted by numeric socket index.
fn decode_available_plugs(
    data: &Map<String, Value>,
    instance_id: &str,
) -> Result<Vec<destiny_core::domain::AvailablePlug>, AppError> {
    let Some(entry) = data.get(instance_id) else {
        return Ok(Vec::new());
    };
    let sockets = entry
        .get("plugs")
        .and_then(Value::as_object)
        .ok_or_else(provider_validation)?;
    if sockets.len() > 32 {
        return Err(provider_validation());
    }
    let mut ordered = Vec::new();
    for (key, entries) in sockets {
        let index = key.parse::<u8>().map_err(|_| provider_validation())?;
        if key != &index.to_string() {
            return Err(provider_validation());
        }
        let entries = entries.as_array().ok_or_else(provider_validation)?;
        ordered.push((index, entries));
    }
    ordered.sort_by_key(|(index, _)| *index);
    let mut plugs = Vec::new();
    for (socket_index, entries) in ordered {
        // Cosmetic sockets can expose hundreds of alternatives. Validate every
        // entry, but omit oversized sockets rather than rejecting the profile or
        // presenting an arbitrary partial selection. The response byte limit
        // still bounds input; retained alternatives remain capped at 64/socket.
        let retain_socket = entries.len() <= 64;
        let mut seen = HashSet::new();
        for entry in entries {
            let hash = decode_required_u32(entry.get("plugItemHash"))?;
            let can_insert = entry
                .get("canInsert")
                .and_then(Value::as_bool)
                .ok_or_else(provider_validation)?;
            let enabled = entry
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or_else(provider_validation)?;
            if retain_socket && can_insert && enabled && seen.insert(hash) {
                plugs.push(destiny_core::domain::AvailablePlug {
                    socket_index,
                    plug_hash: ItemHash::new(i64::from(hash)),
                });
            }
        }
    }
    Ok(plugs)
}

/// Stats are a bounded keyed set, normalized independently of JSON map order.
fn decode_stats(
    data: &Map<String, Value>,
    instance_id: &str,
) -> Result<Vec<StatSummary>, AppError> {
    let Some(entry) = data.get(instance_id) else {
        return Ok(Vec::new());
    };
    let map = entry
        .get("stats")
        .and_then(Value::as_object)
        .ok_or_else(provider_validation)?;
    if map.len() > 64 {
        return Err(provider_validation());
    }
    let mut stats = Vec::with_capacity(map.len());
    let mut seen = HashSet::new();
    for (key, entry) in map {
        let hash = key.parse::<u32>().map_err(|_| provider_validation())?;
        if key != &hash.to_string()
            || !seen.insert(hash)
            || entry.get("statHash").and_then(Value::as_u64) != Some(u64::from(hash))
        {
            return Err(provider_validation());
        }
        let value = entry
            .get("value")
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(provider_validation)?;
        stats.push(StatSummary {
            stat_hash: hash,
            value,
        });
    }
    stats.sort_by_key(|stat| stat.stat_hash);
    Ok(stats)
}

enum RawItem {
    Instanced(RawInstance),
    Stack(StackObservation),
}

/// One retained item before its instance and perk components are resolved.
struct RawInstance {
    transfer_status: Option<u32>,
    instance_id: String,
    item_hash: ItemHash,
    quantity: u32,
    locked: bool,
    location: ItemLocation,
    /// Validated actual bucket, never inferred from item definitions.
    runtime_bucket: Option<u32>,
    /// The equipment status implied by the component the item arrived in.
    equipped: bool,
}

/// The component a decoded item arrived in, which fixes its location and status.
enum ItemOrigin<'a> {
    /// A profile-level item whose location must be decoded from the payload.
    Profile,
    /// A character inventory or equipment item owned by one character.
    Character { id: &'a str, equipped: bool },
}

fn required_component<'a>(
    response: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Value, AppError> {
    response.get(key).ok_or_else(provider_validation)
}

/// Validates one component wrapper and returns its `data` object.
///
/// A wrapper must be an object with an object `data`. It must not be
/// `disabled`, and a present `privacy` value must be `0` (None), `1` (Public),
/// or `2` (Private); any other value is rejected. An absent `privacy` is
/// accepted. A private component is usable only because the owner-authorized
/// response still carries its `data`; one without `data` is rejected below.
fn component_data(value: &Value) -> Result<&Map<String, Value>, AppError> {
    let wrapper = value.as_object().ok_or_else(provider_validation)?;

    if !matches!(wrapper.get("disabled"), None | Some(Value::Bool(false))) {
        return Err(provider_validation());
    }

    if let Some(privacy) = wrapper.get("privacy") {
        let privacy = privacy.as_u64().ok_or_else(provider_validation)?;
        if !matches!(privacy, PRIVACY_NONE | PRIVACY_PUBLIC | PRIVACY_PRIVATE) {
            return Err(provider_validation());
        }
    }

    wrapper
        .get("data")
        .and_then(Value::as_object)
        .ok_or_else(provider_validation)
}

fn decode_characters(value: &Value) -> Result<Vec<Character>, AppError> {
    let data = component_data(value)?;
    let mut characters = Vec::with_capacity(data.len());
    for (key, entry) in data {
        characters.push(decode_character(key, entry)?);
    }
    Ok(characters)
}

fn decode_character(key: &str, value: &Value) -> Result<Character, AppError> {
    validate_decimal_id(key)?;
    let object = value.as_object().ok_or_else(provider_validation)?;

    let character_id = object
        .get("characterId")
        .ok_or_else(provider_validation)
        .and_then(decode_decimal_id_value)?;
    if character_id != key {
        return Err(provider_validation());
    }

    Ok(Character {
        id: CharacterId::new(key),
        // The provider payload is not a trustworthy source of display names.
        name: None,
        class: Some(decode_enum_name(
            object.get("classType"),
            CLASS_NAMES.as_slice(),
        )?),
        race: Some(decode_enum_name(
            object.get("raceType"),
            RACE_NAMES.as_slice(),
        )?),
        gender: Some(decode_enum_name(
            object.get("genderType"),
            GENDER_NAMES.as_slice(),
        )?),
        light: decode_optional_u16(object.get("light"))?,
        date_last_played: object
            .get("dateLastPlayed")
            .filter(|value| !value.is_null())
            .map(|value| -> Result<Timestamp, AppError> {
                let raw = value.as_str().ok_or_else(provider_validation)?;
                let date =
                    chrono::DateTime::parse_from_rfc3339(raw).map_err(|_| provider_validation())?;
                Ok(Timestamp::new(
                    u64::try_from(date.timestamp()).map_err(|_| provider_validation())?,
                ))
            })
            .transpose()?,
    })
}

/// Canonical Destiny class codes. Any other valid `u32` code is preserved as
/// `Unknown(<code>)` rather than guessed.
const CLASS_NAMES: [(u32, &str); 3] = [(0, "Titan"), (1, "Hunter"), (2, "Warlock")];

/// Canonical Destiny race codes. Any other valid `u32` code is preserved as
/// `Unknown(<code>)` rather than guessed.
const RACE_NAMES: [(u32, &str); 3] = [(0, "Human"), (1, "Awoken"), (2, "Exo")];

/// Canonical Destiny gender codes. Any other valid `u32` code is preserved as
/// `Unknown(<code>)` rather than guessed.
const GENDER_NAMES: [(u32, &str); 2] = [(0, "Male"), (1, "Female")];

/// Decodes one required, non-null character enum code.
///
/// A missing or `null` value, a non-numeric value, or a code outside the `u32`
/// range is rejected. A known code maps to its canonical name and every other
/// valid `u32` code is preserved as `Unknown(<code>)`.
fn decode_enum_name(
    value: Option<&Value>,
    names: &'static [(u32, &'static str)],
) -> Result<String, AppError> {
    let code = decode_required_u32(value)?;
    Ok(names
        .iter()
        .find(|(candidate, _)| *candidate == code)
        .map(|(_, name)| (*name).to_owned())
        .unwrap_or_else(|| format!("Unknown({code})")))
}

fn decode_profile_items(value: &Value) -> Result<Vec<RawItem>, AppError> {
    let data = component_data(value)?;
    let items = data
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(provider_validation)?;

    let mut decoded = Vec::with_capacity(items.len());
    for item in items {
        decoded.push(decode_item(item, ItemOrigin::Profile)?);
    }
    Ok(decoded)
}

/// Optional component 103: absent legacy data is not an observed zero balance.
fn decode_profile_currencies(
    response: &Map<String, Value>,
) -> Result<
    (
        destiny_core::ProfileCurrenciesCoverage,
        Vec<StackObservation>,
    ),
    AppError,
> {
    use destiny_core::ProfileCurrenciesCoverage as Coverage;
    let Some(value) = response.get("profileCurrencies") else {
        return Ok((Coverage::NotObserved, Vec::new()));
    };
    if value.is_null() {
        return Ok((Coverage::Unavailable, Vec::new()));
    }
    let wrapper = value.as_object().ok_or_else(provider_validation)?;
    let disabled = match wrapper.get("disabled") {
        None => false,
        Some(Value::Bool(value)) => *value,
        _ => return Err(provider_validation()),
    };
    if let Some(privacy) = wrapper.get("privacy")
        && !matches!(
            privacy.as_u64(),
            Some(PRIVACY_NONE | PRIVACY_PUBLIC | PRIVACY_PRIVATE)
        )
    {
        return Err(provider_validation());
    }
    let Some(data) = wrapper.get("data").filter(|value| !value.is_null()) else {
        return Ok((Coverage::Unavailable, Vec::new()));
    };
    let items = data
        .as_object()
        .and_then(|data| data.get("items"))
        .and_then(Value::as_array)
        .ok_or_else(provider_validation)?;
    let mut stacks = Vec::with_capacity(items.len());
    for value in items {
        let row = value.as_object().ok_or_else(provider_validation)?;
        if !matches!(row.get("itemInstanceId"), None | Some(Value::Null)) {
            return Err(provider_validation());
        }
        let bucket = decode_required_u32(row.get("bucketHash"))?;
        let location = row
            .get("location")
            .map(|v| decode_required_u32(Some(v)))
            .transpose()?;
        if bucket == LOST_ITEMS_BUCKET_HASH || location == Some(LOCATION_POSTMASTER) {
            return Err(provider_validation());
        }
        let quantity = row
            .get("quantity")
            .and_then(Value::as_i64)
            .ok_or_else(provider_validation)?;
        stacks.push(StackObservation::new(
            decode_item_hash(row.get("itemHash"))?,
            BucketHash::new(i64::from(bucket)),
            quantity,
            StackContainer::ProfileCurrencies,
        )?);
    }
    if disabled {
        Ok((Coverage::Unavailable, Vec::new()))
    } else {
        Ok((Coverage::Available, stacks))
    }
}

fn decode_character_items(
    value: &Value,
    character_ids: &BTreeSet<String>,
    equipped: bool,
) -> Result<Vec<RawItem>, AppError> {
    let data = component_data(value)?;
    let mut present = HashSet::with_capacity(data.len());
    let mut decoded = Vec::new();

    for (character_id, group) in data {
        if !character_ids.contains(character_id) {
            return Err(provider_validation());
        }
        present.insert(character_id.as_str());

        let group = group.as_object().ok_or_else(provider_validation)?;
        let items = group
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(provider_validation)?;
        for item in items {
            let origin = ItemOrigin::Character {
                id: character_id,
                equipped,
            };
            decoded.push(decode_item(item, origin)?);
        }
    }

    if character_ids
        .iter()
        .any(|character_id| !present.contains(character_id.as_str()))
    {
        return Err(provider_validation());
    }

    Ok(decoded)
}

fn decode_item(value: &Value, origin: ItemOrigin<'_>) -> Result<RawItem, AppError> {
    let object = value.as_object().ok_or_else(provider_validation)?;
    // Validate supplied runtime buckets even for legacy instanced gear; absence
    // remains supported when classification does not depend on the bucket.
    let runtime_bucket = object
        .get("bucketHash")
        .map(|value| decode_required_u32(Some(value)))
        .transpose()?;
    let postmaster_bucket = runtime_bucket == Some(LOST_ITEMS_BUCKET_HASH);
    let (location, container, equipped) = match origin {
        ItemOrigin::Profile => {
            let provider_location = decode_required_u32(object.get("location"))?;
            // This component supplies no validated character owner.
            if provider_location == LOCATION_POSTMASTER || postmaster_bucket {
                return Err(provider_validation());
            }
            let location = if provider_location == LOCATION_VAULT {
                ItemLocation::Vault
            } else {
                ItemLocation::other(format!("bungie-profile-location:{provider_location}"))
            };
            (location, StackContainer::ProfileInventory, false)
        }
        ItemOrigin::Character { id, equipped } => {
            let owner = CharacterId::new(id);
            // Existing character gear accepts absent location; source ownership
            // remains authoritative. A supplied location must be a valid u32.
            let provider_location = object
                .get("location")
                .map(|value| decode_required_u32(Some(value)))
                .transpose()?;
            if provider_location == Some(LOCATION_POSTMASTER) || postmaster_bucket {
                if equipped {
                    return Err(provider_validation());
                }
                (
                    ItemLocation::Postmaster(owner.clone()),
                    StackContainer::Postmaster(owner),
                    false,
                )
            } else {
                (
                    ItemLocation::Character(owner.clone()),
                    StackContainer::CharacterInventory(owner),
                    equipped,
                )
            }
        }
    };

    let item_hash = decode_item_hash(object.get("itemHash"))?;
    let instance_id = match object.get("itemInstanceId") {
        None | Some(Value::Null) => {
            if equipped {
                return Err(provider_validation());
            }
            let bucket = decode_required_u32(object.get("bucketHash"))?;
            let quantity = object
                .get("quantity")
                .and_then(Value::as_i64)
                .ok_or_else(provider_validation)?;
            return StackObservation::new(
                item_hash,
                BucketHash::new(i64::from(bucket)),
                quantity,
                container,
            )
            .map(RawItem::Stack);
        }
        Some(value) => decode_decimal_id_value(value)?,
    };
    let quantity = decode_quantity(object.get("quantity"))?;
    let state = decode_state(object.get("state"))?;
    let locked = state & 1 != 0;
    Ok(RawItem::Instanced(RawInstance {
        transfer_status: object
            .get("transferStatus")
            .filter(|v| !v.is_null())
            .map(|v| decode_required_u32(Some(v)))
            .transpose()?,
        instance_id,
        item_hash,
        quantity,
        locked,
        location,
        runtime_bucket,
        equipped,
    }))
}

fn decode_item_hash(value: Option<&Value>) -> Result<ItemHash, AppError> {
    // Bungie item hashes are unsigned 32-bit values.
    let hash = decode_required_u32(value)?;
    Ok(ItemHash::new(i64::from(hash)))
}

fn decode_quantity(value: Option<&Value>) -> Result<u32, AppError> {
    let raw = value
        .and_then(Value::as_u64)
        .ok_or_else(provider_validation)?;
    u32::try_from(raw).map_err(|_| provider_validation())
}

fn decode_state(value: Option<&Value>) -> Result<u64, AppError> {
    let raw = value
        .and_then(Value::as_u64)
        .ok_or_else(provider_validation)?;
    if raw > u64::from(u32::MAX) {
        return Err(provider_validation());
    }
    Ok(raw)
}

/// Decodes one required, non-null, unsigned 32-bit provider value.
fn decode_required_u32(value: Option<&Value>) -> Result<u32, AppError> {
    let raw = value
        .and_then(Value::as_u64)
        .ok_or_else(provider_validation)?;
    u32::try_from(raw).map_err(|_| provider_validation())
}

/// Decodes an optional unsigned `u16`, rejecting wrong types and out-of-range values.
fn decode_optional_u16(value: Option<&Value>) -> Result<Option<u16>, AppError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => {
            let raw = number.as_u64().ok_or_else(provider_validation)?;
            let value = u16::try_from(raw).map_err(|_| provider_validation())?;
            Ok(Some(value))
        }
        Some(_) => Err(provider_validation()),
    }
}

/// Decodes the optional power from `primaryStat.value`.
///
/// Live profiles include items whose primary stat is an integer outside the
/// power range (a negative value has been observed), so any integer that does
/// not fit `u16` is an unknown power rather than a malformed response. A
/// non-integer value or a non-object `primaryStat` is still rejected.
fn decode_power(instance: &Map<String, Value>) -> Result<Option<u16>, AppError> {
    match instance.get("primaryStat") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(stat)) => match stat.get("value") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(number)) if number.is_i64() || number.is_u64() => {
                Ok(number.as_u64().and_then(|raw| u16::try_from(raw).ok()))
            }
            Some(_) => Err(provider_validation()),
        },
        Some(_) => Err(provider_validation()),
    }
}

fn optional_u32(object: &Map<String, Value>, key: &str) -> Result<Option<u32>, AppError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => u32::try_from(value.as_u64().ok_or_else(provider_validation)?)
            .map(Some)
            .map_err(|_| provider_validation()),
    }
}

fn decode_energy(
    instance: &Map<String, Value>,
) -> Result<Option<destiny_core::ItemEnergy>, AppError> {
    let energy = match instance.get("energy") {
        None | Some(Value::Null) => return Ok(None),
        Some(value) => value.as_object().ok_or_else(provider_validation)?,
    };
    Ok(Some(destiny_core::ItemEnergy {
        energy_capacity: optional_u32(energy, "energyCapacity")?.ok_or_else(provider_validation)?,
        energy_used: optional_u32(energy, "energyUsed")?.ok_or_else(provider_validation)?,
        energy_unused: optional_u32(energy, "energyUnused")?.ok_or_else(provider_validation)?,
        energy_type: optional_u32(energy, "energyType")?,
        energy_type_hash: optional_u32(energy, "energyTypeHash")?,
    }))
}

/// Retain raw array positions and flags independently of publication's plug filters.
fn decode_sockets(
    sockets: &Map<String, Value>,
    instance_id: &str,
) -> Result<Vec<destiny_core::IndexedSocket>, AppError> {
    let Some(entry) = sockets.get(instance_id) else {
        return Ok(Vec::new());
    };
    let entries = entry
        .as_object()
        .ok_or_else(provider_validation)?
        .get("sockets")
        .and_then(Value::as_array)
        .ok_or_else(provider_validation)?;
    if entries.len() > MAX_SOCKETS_PER_ITEM {
        return Err(provider_validation());
    }
    let mut indexed = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let socket = entry.as_object().ok_or_else(provider_validation)?;
        let enabled = socket
            .get("isEnabled")
            .and_then(Value::as_bool)
            .ok_or_else(provider_validation)?;
        // Visibility is validated but never controls whether a selected plug is retained.
        let visible = socket
            .get("isVisible")
            .and_then(Value::as_bool)
            .ok_or_else(provider_validation)?;
        let hash = match socket.get("plugHash") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                u32::try_from(value.as_u64().ok_or_else(provider_validation)?)
                    .map_err(|_| provider_validation())?,
            ),
        };
        indexed.push(destiny_core::IndexedSocket {
            socket_index: u8::try_from(index).map_err(|_| provider_validation())?,
            plug_hash: hash.map(|hash| ItemHash::new(i64::from(hash))),
            enabled,
            visible,
        });
    }
    Ok(indexed)
}

fn decode_perks(
    perks: &Map<String, Value>,
    instance_id: &str,
) -> Result<Vec<PerkSummary>, AppError> {
    let Some(entry) = perks.get(instance_id) else {
        return Ok(Vec::new());
    };
    let entry = entry.as_object().ok_or_else(provider_validation)?;
    let summaries = entry
        .get("perks")
        .and_then(Value::as_array)
        .ok_or_else(provider_validation)?;

    let mut decoded = Vec::with_capacity(summaries.len());
    for summary in summaries {
        decoded.push(decode_perk(summary)?);
    }
    Ok(decoded)
}

fn decode_perk(value: &Value) -> Result<PerkSummary, AppError> {
    let object = value.as_object().ok_or_else(provider_validation)?;

    let perk_hash = object
        .get("perkHash")
        .and_then(Value::as_u64)
        .ok_or_else(provider_validation)?;
    let perk_hash = u32::try_from(perk_hash).map_err(|_| provider_validation())?;

    let active = object
        .get("isActive")
        .and_then(Value::as_bool)
        .ok_or_else(provider_validation)?;

    let visible = object
        .get("visible")
        .and_then(Value::as_bool)
        .ok_or_else(provider_validation)?;

    Ok(PerkSummary {
        perk_hash: PerkHash::new(perk_hash),
        active,
        visible,
    })
}

/// Requires a non-empty, canonical, unsigned decimal identifier of at most 128
/// digits.
///
/// The bound matches the durable profile projection, so an identifier accepted
/// here can always be persisted. Only ASCII digits are accepted; signs,
/// whitespace, separators, and any non-ASCII digit are rejected. Leading zeros
/// are rejected as non-canonical, so `"0"` is the only accepted identifier
/// with a leading zero. Canonical decimal form is required because characters
/// and items are ordered by length first and then lexicographically, which is
/// numeric order only for canonical decimal strings.
fn validate_decimal_id(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_DIGITS
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(provider_validation());
    }
    Ok(())
}

/// Decodes a decimal identifier that the provider may serialize as either a
/// decimal string or an exact nonnegative JSON integer.
///
/// Real Bungie payloads serialize identifiers such as `characterId` as decimal
/// strings, so string support is mandatory. The generated schema may instead
/// emit an unsigned integer, so an exact nonnegative `u64` is accepted and
/// canonicalized to its decimal representation before use. Floating-point and
/// negative numbers are never accepted, because `Value::as_u64` returns `None`
/// for them; no `f64` or `i64` conversion is performed anywhere in this path.
fn decode_decimal_id_value(value: &Value) -> Result<String, AppError> {
    match value {
        Value::String(text) => {
            validate_decimal_id(text)?;
            Ok(text.clone())
        }
        Value::Number(number) => {
            let raw = number.as_u64().ok_or_else(provider_validation)?;
            Ok(raw.to_string())
        }
        _ => Err(provider_validation()),
    }
}

/// Ascending numeric order for canonical decimal identifiers without parsing
/// them, so identifiers wider than any integer type still compare correctly.
fn decimal_id_order(left: &str, right: &str) -> Ordering {
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

#[cfg(test)]
mod tests {
    use super::decode_profile;
    use destiny_core::{
        AppError, Character, CharacterId, DefinitionVersion, DestinyMembership, ErrorCategory,
        InventorySnapshotInput, ItemHash, ItemInstance, ItemInstanceId, ItemLocation, MembershipId,
        MembershipType, PerkHash, PerkSummary, PlugSummary, SourceMarker, Timestamp,
    };
    use serde_json::{Value, json};

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/profile-complete.json");
    const MEMBERSHIP_TYPE: u32 = 3;
    const MEMBERSHIP_ID: &str = "4611686018484639825";
    const DEFINITION_VERSION: &str = "manifest-fixture-v1";
    const CHECKED_AT: u64 = 1_724_000_000;

    fn membership() -> DestinyMembership {
        DestinyMembership::new(
            MembershipType::new(MEMBERSHIP_TYPE),
            MembershipId::new(MEMBERSHIP_ID),
        )
        .expect("fixture membership")
    }

    fn decode(bytes: &[u8]) -> Result<InventorySnapshotInput, AppError> {
        decode_profile(
            bytes,
            &membership(),
            DefinitionVersion::new(DEFINITION_VERSION),
            Timestamp::new(CHECKED_AT),
        )
    }

    fn observed(bytes: &[u8]) -> Result<destiny_core::ProfileAcquisition, AppError> {
        super::decode_profile_with_observations(
            bytes,
            &membership(),
            DefinitionVersion::new(DEFINITION_VERSION),
            Timestamp::new(CHECKED_AT),
        )
    }

    #[test]
    fn saved_loadouts_decode_present_absent_private_and_malformed() {
        let mut value = fixture_value();
        assert_eq!(
            observed(&serde_json::to_vec(&value).unwrap())
                .unwrap()
                .saved_loadouts(),
            None
        );
        for component in [
            serde_json::json!({"privacy":2}),
            serde_json::json!({"data":null}),
            serde_json::json!({"data":"invalid"}),
        ] {
            value["Response"]["characterLoadouts"] = component;
            assert_eq!(
                observed(&serde_json::to_vec(&value).unwrap())
                    .unwrap()
                    .saved_loadouts(),
                None
            );
        }
        value["Response"]["characterLoadouts"] = serde_json::json!({"privacy":2,"data":{"1":{"loadouts":[{"nameHash":10,"colorHash":20,"iconHash":30,"items":[{"itemInstanceId":"456","plugItemHashes":[42,2166136261u32]}]}]}}});
        let pair = observed(&serde_json::to_vec(&value).unwrap()).unwrap();
        let rows = pair.saved_loadouts().unwrap();
        assert_eq!(rows[0].loadouts[0].name_hash, 10);
        assert_eq!(rows[0].loadouts[0].items[0].plugs.len(), 1);
    }

    #[test]
    fn minted_time_decode_preserves_fraction_and_ignores_unusable_values() {
        let mut value = fixture_value();
        value["Response"]
            .as_object_mut()
            .unwrap()
            .remove("responseMintedTimestamp");
        assert_eq!(
            observed(&serde_json::to_vec(&value).unwrap())
                .unwrap()
                .minted_at(),
            None
        );
        for (text, nanos) in [
            ("2026-10-08T21:09:13.6826469Z", 682646900),
            ("2026-10-08T21:09:13.123456789Z", 123456789),
            ("2026-10-08T23:09:13.123456789+02:00", 123456789),
        ] {
            value["Response"]["responseMintedTimestamp"] = json!(text);
            let pair = observed(&serde_json::to_vec(&value).unwrap()).unwrap();
            let minted = pair.minted_at().expect("present instant");
            assert_eq!(minted.nanoseconds(), nanos);
            assert_eq!(
                minted.seconds(),
                chrono::DateTime::parse_from_rfc3339("2026-10-08T21:09:13Z")
                    .unwrap()
                    .timestamp()
            );
        }
        for malformed in [
            json!("not-a-time"),
            json!(null),
            json!(42),
            json!("2026-10-08T21:09:13.1234567891Z"),
            json!("2016-12-31T23:59:60Z"),
        ] {
            // Ordering evidence only: an unusable value never fails the sync.
            value["Response"]["responseMintedTimestamp"] = malformed;
            assert_eq!(
                observed(&serde_json::to_vec(&value).unwrap())
                    .unwrap()
                    .minted_at(),
                None
            );
        }
    }

    fn bucket_fixture() -> Value {
        let mut value = fixture_value();
        let response = value["Response"].as_object_mut().unwrap();
        for row in response["profileInventory"]["data"]["items"]
            .as_array_mut()
            .unwrap()
        {
            if row["itemInstanceId"].is_string() {
                row["bucketHash"] = json!(123);
            }
        }
        for group in ["characterInventories", "characterEquipment"] {
            for owner in response[group]["data"]
                .as_object_mut()
                .unwrap()
                .values_mut()
            {
                for row in owner["items"].as_array_mut().unwrap() {
                    if row["itemInstanceId"].is_string() {
                        row["bucketHash"] = json!(123);
                    }
                }
            }
        }
        value
    }

    #[test]
    fn observation_decoder_complete_actual_buckets_preserves_input() {
        let mut value = bucket_fixture();
        // Equal definition hashes must not collapse distinct runtime instances.
        value["Response"]["profileInventory"]["data"]["items"][1]["itemHash"] = json!(1001);
        let bytes = serde_json::to_vec(&value).unwrap();
        let pair = observed(&bytes).unwrap();
        assert_eq!(pair.input(), &decode(&bytes).unwrap());
        let rows = pair.observations().unwrap();
        assert_eq!(rows.len(), 6);
        for item in &pair.input().items {
            let row = rows
                .iter()
                .find(|row| row.instance_id() == &item.instance_id)
                .unwrap();
            assert_eq!(row.location(), &item.location);
            assert_eq!(row.bucket_hash().value(), 123);
        }
    }

    #[test]
    fn observation_decoder_missing_discards_partial_proof_and_zero_is_complete() {
        let mut value = bucket_fixture();
        value["Response"]["profileCurrencies"] = json!({"privacy": 2, "data": {"items": [
            {"itemHash": 900, "bucketHash": 99, "quantity": 0},
            {"itemHash": 900, "bucketHash": 99, "quantity": 0}
        ]}});
        value["Response"]["profileInventory"]["data"]["items"][0]
            .as_object_mut()
            .unwrap()
            .remove("bucketHash");
        let bytes = serde_json::to_vec(&value).unwrap();
        let pair = observed(&bytes).unwrap();
        assert!(pair.observations().is_none());
        assert_eq!(pair.input(), &decode(&bytes).unwrap());
        assert_eq!(pair.input().stacks.len(), 5);
        let response = value["Response"].as_object_mut().unwrap();
        response["profileInventory"]["data"]["items"]
            .as_array_mut()
            .unwrap()
            .retain(|row| !row["itemInstanceId"].is_string());
        for group in ["characterInventories", "characterEquipment"] {
            for owner in response[group]["data"]
                .as_object_mut()
                .unwrap()
                .values_mut()
            {
                owner["items"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|row| !row["itemInstanceId"].is_string());
            }
        }
        let bytes = serde_json::to_vec(&value).unwrap();
        let pair = observed(&bytes).unwrap();
        assert_eq!(pair.input(), &decode(&bytes).unwrap());
        assert!(pair.input().items.is_empty());
        assert_eq!(pair.observations(), Some([].as_slice()));
        assert_eq!(pair.input().stacks.len(), 5);
    }

    #[test]
    fn observation_decoder_bucket_width_types_and_duplicate_keys_fail_atomically() {
        for invalid in [
            Value::Null,
            json!(-1),
            json!(4294967296_u64),
            json!(1.5),
            json!(true),
            json!("123"),
            json!([]),
            json!({}),
        ] {
            let mut value = bucket_fixture();
            value["Response"]["profileInventory"]["data"]["items"][0]["bucketHash"] = invalid;
            let bytes = serde_json::to_vec(&value).unwrap();
            assert_eq!(
                observed(&bytes).unwrap_err().category(),
                ErrorCategory::ProviderValidation
            );
            assert_validation(decode(&bytes));
        }
        let bytes = serde_json::to_vec(&bucket_fixture()).unwrap();
        let duplicate = String::from_utf8(bytes).unwrap().replacen(
            "\"bucketHash\":123",
            "\"bucketHash\":123,\"bucketHash\":123",
            1,
        );
        assert_validation(decode(duplicate.as_bytes()));
        assert_eq!(
            observed(duplicate.as_bytes()).unwrap_err().category(),
            ErrorCategory::ProviderValidation
        );
        for bucket in [0_u32, u32::MAX] {
            let mut value = bucket_fixture();
            value["Response"]["profileInventory"]["data"]["items"][0]["bucketHash"] = json!(bucket);
            let bytes = serde_json::to_vec(&value).unwrap();
            let pair = observed(&bytes).unwrap();
            assert_eq!(pair.input(), &decode(&bytes).unwrap());
            let row = pair
                .observations()
                .unwrap()
                .iter()
                .find(|row| row.instance_id().as_str() == "700000000000000001")
                .unwrap();
            assert_eq!(row.bucket_hash().value(), i64::from(bucket));
        }
    }

    #[test]
    fn observation_decoder_owned_postmaster_and_bad_joins() {
        let mut value = bucket_fixture();
        value["Response"]["characterInventories"]["data"]["900000000000000001"]["items"][0]["bucketHash"] =
            json!(super::LOST_ITEMS_BUCKET_HASH);
        let bytes = serde_json::to_vec(&value).unwrap();
        let pair = observed(&bytes).unwrap();
        let row = pair
            .observations()
            .unwrap()
            .iter()
            .find(|row| row.instance_id().as_str() == "700000000000000004")
            .unwrap();
        assert_eq!(
            row.location(),
            &ItemLocation::Postmaster(CharacterId::new("900000000000000001"))
        );
        assert_eq!(pair.input(), &decode(&bytes).unwrap());
        for bad_owner in [false, true] {
            let mut value = bucket_fixture();
            if bad_owner {
                let owners = value["Response"]["characterInventories"]["data"]
                    .as_object_mut()
                    .unwrap();
                let items = owners.remove("900000000000000001").unwrap();
                owners.insert("999".to_owned(), items);
            } else {
                value["Response"]["profileInventory"]["data"]["items"][1]["itemInstanceId"] =
                    json!("700000000000000001");
            }
            let bytes = serde_json::to_vec(&value).unwrap();
            assert_validation(decode(&bytes));
            assert_eq!(
                observed(&bytes).unwrap_err().category(),
                ErrorCategory::ProviderValidation
            );
        }
        for component in [
            "characters",
            "profileInventory",
            "characterInventories",
            "characterEquipment",
        ] {
            let mut value = bucket_fixture();
            value["Response"][component]["disabled"] = json!(true);
            let bytes = serde_json::to_vec(&value).unwrap();
            assert_validation(decode(&bytes));
            assert_eq!(
                observed(&bytes).unwrap_err().category(),
                ErrorCategory::ProviderValidation
            );
        }
        let mut value = bucket_fixture();
        value["Response"]["itemComponents"]["instances"]["data"]["700000000000000001"] =
            Value::Null;
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_validation(decode(&bytes));
        assert_eq!(
            observed(&bytes).unwrap_err().category(),
            ErrorCategory::ProviderValidation
        );
    }

    #[test]
    fn character_last_played_normalizes_dates_and_rejects_invalid_values() {
        use super::decode_character;
        let value = serde_json::json!({
            "characterId": "1", "classType": 1, "raceType": 0, "genderType": 0,
            "dateLastPlayed": "2024-01-01T01:00:00+01:00"
        });
        assert_eq!(
            decode_character("1", &value).unwrap().date_last_played,
            Some(Timestamp::new(1704067200))
        );
        for invalid in [
            serde_json::json!(42),
            serde_json::json!("bad"),
            serde_json::json!("1960-01-01T00:00:00Z"),
        ] {
            let mut value = value.clone();
            value["dateLastPlayed"] = invalid;
            assert_eq!(
                decode_character("1", &value).unwrap_err().category(),
                ErrorCategory::ProviderValidation
            );
        }
        let mut value = value;
        value.as_object_mut().unwrap().remove("dateLastPlayed");
        assert!(
            decode_character("1", &value)
                .unwrap()
                .date_last_played
                .is_none()
        );
        value["dateLastPlayed"] = Value::Null;
        assert!(
            decode_character("1", &value)
                .unwrap()
                .date_last_played
                .is_none()
        );
    }

    #[test]
    fn profile_currency_coverage_and_zero_balance() {
        use destiny_core::{ProfileCurrenciesCoverage as Coverage, StackContainer};
        assert_eq!(
            decode(FIXTURE).unwrap().profile_currencies_coverage,
            Coverage::NotObserved
        );
        let snapshot = decode_mutated(|v| {
            v["Response"]["profileCurrencies"] = json!({"privacy":2,"data":{"items":[
                {"itemHash":42,"bucketHash":4100,"quantity":0},
                {"itemHash":42,"bucketHash":4100,"quantity":0}
            ]}});
        })
        .unwrap();
        assert_eq!(snapshot.profile_currencies_coverage, Coverage::Available);
        assert_eq!(snapshot.stacks.len(), 5);
        assert_eq!(
            snapshot.stacks[3].container(),
            &StackContainer::ProfileCurrencies
        );
        assert_eq!(snapshot.stacks[3].quantity(), 0);
        assert_eq!(snapshot.stacks[3], snapshot.stacks[4]);
    }

    #[test]
    fn profile_currency_wrapper_availability_is_not_empty_holdings() {
        use destiny_core::ProfileCurrenciesCoverage as Coverage;
        for wrapper in [
            Value::Null,
            json!({}),
            json!({"data":null}),
            json!({"privacy":2}),
            json!({"disabled":true}),
            json!({"disabled":true,"data":{"items":[]}}),
        ] {
            let snapshot =
                decode_mutated(|v| v["Response"]["profileCurrencies"] = wrapper).unwrap();
            assert_eq!(snapshot.profile_currencies_coverage, Coverage::Unavailable);
            assert_eq!(snapshot.stacks.len(), 3);
            assert_eq!(snapshot.items.len(), decode(FIXTURE).unwrap().items.len());
        }
        for privacy in [0, 1, 2] {
            let snapshot = decode_mutated(|v| {
                v["Response"]["profileCurrencies"] = json!({"privacy":privacy,"data":{"items":[]}})
            })
            .unwrap();
            assert_eq!(snapshot.profile_currencies_coverage, Coverage::Available);
            assert_eq!(snapshot.stacks.len(), 3);
        }
        for wrapper in [
            json!(false),
            json!([]),
            json!({"disabled":null}),
            json!({"disabled":1}),
            json!({"privacy":3}),
            json!({"privacy":null}),
            json!({"privacy":"2"}),
            json!({"disabled":true,"privacy":false}),
            json!({"data":[]}),
            json!({"data":{}}),
            json!({"data":{"items":null}}),
            json!({"disabled":true,"data":false}),
        ] {
            assert_validation(decode_mutated(|v| {
                v["Response"]["profileCurrencies"] = wrapper
            }));
        }
    }

    #[test]
    fn profile_currency_rows_are_strict_and_keep_overlap_provenance() {
        use destiny_core::{ProfileCurrenciesCoverage as Coverage, StackContainer};
        let row = json!({"itemHash":123,"bucketHash":4100,"quantity":2147483647,"location":2});
        let snapshot = decode_mutated(|v| {
            v["Response"]["profileInventory"]["data"]["items"]
                .as_array_mut()
                .unwrap()
                .push(row.clone());
            v["Response"]["profileCurrencies"] =
                json!({"data":{"items":[row.clone(), row.clone()]}});
        })
        .unwrap();
        assert_eq!(snapshot.profile_currencies_coverage, Coverage::Available);
        let overlap: Vec<_> = snapshot
            .stacks
            .iter()
            .filter(|s| s.item_hash().value() == 123)
            .collect();
        assert_eq!(overlap.len(), 3);
        assert_eq!(overlap[0].container(), &StackContainer::ProfileInventory);
        assert_eq!(overlap[1].container(), &StackContainer::ProfileCurrencies);
        assert_eq!(overlap[2], overlap[1]);
        for (field, invalid) in [
            ("itemHash", json!(-1)),
            ("itemHash", json!(4294967296u64)),
            ("bucketHash", json!(-1)),
            ("bucketHash", json!(4294967296u64)),
            ("bucketHash", Value::Null),
            ("quantity", json!(-1)),
            ("quantity", json!(2147483648u64)),
            ("quantity", json!(1.5)),
            ("quantity", Value::Null),
            ("itemInstanceId", json!("0")),
            ("itemInstanceId", json!("123")),
            ("location", json!(4)),
            ("bucketHash", json!(215593132)),
            ("location", json!("1")),
        ] {
            let mut bad = row.clone();
            bad[field] = invalid;
            assert_validation(decode_mutated(|v| {
                v["Response"]["profileCurrencies"] = json!({"data":{"items":[bad]}})
            }));
        }
        for field in ["itemHash", "bucketHash", "quantity"] {
            let mut bad = row.clone();
            bad.as_object_mut().unwrap().remove(field);
            assert_validation(decode_mutated(|v| {
                v["Response"]["profileCurrencies"] = json!({"data":{"items":[bad]}})
            }));
        }
    }

    #[test]
    fn profile_bucket_classification_rejects_equipped_unowned_and_invalid_buckets() {
        for component in ["profileInventory", "characterEquipment"] {
            assert_validation(decode_mutated(|v| {
                let data = &mut v["Response"][component]["data"];
                let rows = if component == "profileInventory" {
                    &mut data["items"]
                } else {
                    &mut data["900000000000000001"]["items"]
                };
                rows[0]["bucketHash"] = json!(215593132);
            }));
        }
        for invalid in [
            Value::Null,
            json!(-1),
            json!(4294967296u64),
            json!("215593132"),
            json!(false),
        ] {
            assert_validation(decode_mutated(|v| {
                v["Response"]["characterInventories"]["data"]["900000000000000001"]["items"][0]["bucketHash"] =
                    invalid
            }));
        }
    }

    #[test]
    fn profile_bucket_only_postmaster() {
        use destiny_core::StackContainer;
        for location in [None, Some(json!(1))] {
            let snapshot = decode_mutated(|v| {
                let rows =
                    v["Response"]["characterInventories"]["data"]["900000000000000001"]["items"]
                        .as_array_mut()
                        .unwrap();
                for row in rows {
                    row["bucketHash"] = json!(215593132);
                    row.as_object_mut().unwrap().remove("location");
                    if let Some(location) = &location {
                        row["location"] = location.clone();
                    }
                }
            })
            .unwrap();
            let owner = character_id("900000000000000001");
            assert_eq!(
                item(&snapshot, "700000000000000004").location,
                ItemLocation::Postmaster(owner.clone())
            );
            assert_eq!(
                snapshot.stacks[2].container(),
                &StackContainer::Postmaster(owner)
            );
        }
    }

    #[test]
    fn profile_retains_synthetic_inventory_stacks_without_instance_components() {
        use destiny_core::StackContainer;
        let snapshot = decode(FIXTURE).unwrap();
        assert_eq!(snapshot.stacks.len(), 3);
        assert_eq!(
            snapshot.stacks[0].container(),
            &StackContainer::ProfileInventory
        );
        assert_eq!(snapshot.stacks[0].quantity(), 99);
        assert_eq!(snapshot.stacks[0].bucket_hash().value(), 4100);
        assert_eq!(
            snapshot.stacks[2].container(),
            &StackContainer::CharacterInventory(character_id("900000000000000001"))
        );
    }

    #[test]
    fn profile_character_postmaster_uses_validated_owner_for_gear_and_stacks() {
        use destiny_core::StackContainer;
        let snapshot = decode_mutated(|value| {
            let rows = &mut value["Response"]["characterInventories"]["data"]["900000000000000001"]
                ["items"];
            rows[0]["location"] = json!(4);
            rows[1]["location"] = json!(4);
        })
        .unwrap();
        let owner = character_id("900000000000000001");
        assert_eq!(
            item(&snapshot, "700000000000000004").location,
            ItemLocation::Postmaster(owner.clone())
        );
        assert_eq!(
            snapshot.stacks[2].container(),
            &StackContainer::Postmaster(owner)
        );
    }

    #[test]
    fn profile_stack_fields_are_strict_and_accept_full_hash_width() {
        for field in ["itemHash", "bucketHash", "quantity"] {
            for invalid in [
                Value::Null,
                json!(-1),
                json!(1.5),
                json!("1"),
                json!(u64::MAX),
            ] {
                assert_validation(decode_mutated(set_profile_item_field(2, field, invalid)));
            }
            assert_validation(decode_mutated(|value| {
                value["Response"]["profileInventory"]["data"]["items"][2]
                    .as_object_mut()
                    .unwrap()
                    .remove(field);
            }));
        }
        for quantity in [0i64, i64::from(i32::MAX) + 1] {
            assert_validation(decode_mutated(set_profile_item_field(
                2,
                "quantity",
                json!(quantity),
            )));
        }
        let snapshot = decode_mutated(|value| {
            let row = &mut value["Response"]["profileInventory"]["data"]["items"][2];
            row["itemHash"] = json!(0);
            row["bucketHash"] = json!(u32::MAX);
            row["quantity"] = json!(i32::MAX);
        })
        .unwrap();
        assert_eq!(snapshot.stacks[0].item_hash().value(), 0);
        assert_eq!(
            snapshot.stacks[0].bucket_hash().value(),
            i64::from(u32::MAX)
        );
        assert_eq!(snapshot.stacks[0].quantity(), i32::MAX);
    }

    #[test]
    fn profile_postmaster_rejects_unknown_owner_and_equipped_rows() {
        assert_validation(decode_mutated(|value| {
            let groups = value["Response"]["characterInventories"]["data"]
                .as_object_mut()
                .unwrap();
            let mut group = groups.remove("900000000000000001").unwrap();
            group["items"][0]["location"] = json!(4);
            groups.insert("999".into(), group);
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["characterEquipment"]["data"]["900000000000000001"]["items"][0]["location"] =
                json!(4);
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["characterInventories"]["data"]["900000000000000001"]["items"][0]["location"] =
                json!(4);
            value["Response"]["itemComponents"]["instances"]["data"]["700000000000000004"]["isEquipped"] =
                json!(true);
        }));
    }

    #[test]
    fn profile_character_gear_preserves_absent_location_but_validates_supplied_location() {
        let snapshot = decode_mutated(|value| {
            value["Response"]["characterInventories"]["data"]["900000000000000001"]["items"][0]
                .as_object_mut()
                .unwrap()
                .remove("location");
        })
        .unwrap();
        assert_eq!(
            item(&snapshot, "700000000000000004").location,
            ItemLocation::Character(character_id("900000000000000001"))
        );
        for invalid in [Value::Null, json!(-1), json!(4294967296u64)] {
            assert_validation(decode_mutated(|value| {
                value["Response"]["characterInventories"]["data"]["900000000000000001"]["items"]
                    [0]["location"] = invalid;
            }));
        }
    }

    #[test]
    fn profile_literal_zero_still_requires_a_real_instance_component() {
        assert_validation(decode_mutated(set_profile_item_field(
            2,
            "itemInstanceId",
            json!("0"),
        )));
        let snapshot = decode_mutated(|value| {
            value["Response"]["profileInventory"]["data"]["items"][2]["itemInstanceId"] =
                json!("0");
            value["Response"]["itemComponents"]["instances"]["data"]["0"] =
                json!({"isEquipped": false});
        })
        .unwrap();
        assert_eq!(snapshot.items[0].instance_id.as_str(), "0");
        assert_eq!(snapshot.stacks.len(), 2);
    }

    #[test]
    fn transfer_status_is_retained_and_absence_is_unknown() {
        let absent = decode_mutated(|_| {}).unwrap();
        assert_eq!(absent.items[0].transfer_status, None);
        for status in [0_u32, 1, 2, 3, 4, u32::MAX] {
            let input = decode_mutated(|value| {
                value["Response"]["profileInventory"]["data"]["items"][0]["transferStatus"] =
                    json!(status);
            })
            .unwrap();
            assert_eq!(
                input
                    .items
                    .iter()
                    .find(|i| i.instance_id.as_str() == "700000000000000001")
                    .unwrap()
                    .transfer_status,
                Some(status)
            );
        }
        for invalid in [json!(-1), json!(4294967296_u64), json!(true), json!("1")] {
            assert_validation(decode_mutated(|value| {
                value["Response"]["profileInventory"]["data"]["items"][0]["transferStatus"] =
                    invalid;
            }));
        }
    }

    fn fixture_value() -> Value {
        serde_json::from_slice(FIXTURE).expect("fixture is JSON")
    }

    fn decode_mutated(mutate: impl FnOnce(&mut Value)) -> Result<InventorySnapshotInput, AppError> {
        let mut value = fixture_value();
        mutate(&mut value);
        let bytes = serde_json::to_vec(&value).expect("mutated fixture serializes");
        decode(&bytes)
    }

    fn assert_validation(result: Result<InventorySnapshotInput, AppError>) {
        let error = result.expect_err("expected a provider-validation failure");
        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
    }

    fn character_id(value: &str) -> CharacterId {
        CharacterId::new(value)
    }

    fn instance_id(value: &str) -> ItemInstanceId {
        ItemInstanceId::new(value)
    }

    #[test]
    fn reusable_plugs_are_sorted_filtered_deduplicated_and_bounded() {
        let plug = |hash, can_insert, enabled| json!({"plugItemHash": hash, "canInsert": can_insert, "enabled": enabled});
        let snapshot = decode_mutated(|value| {
            value["Response"]["itemComponents"]["reusablePlugs"] = json!({"privacy": 2, "data": {"700000000000000001": {"plugs": {"10": [plug(12, true, true)], "2": [plug(11, true, true), plug(11, true, true), plug(13, false, true), plug(14, true, false)]}}}});
        }).unwrap();
        assert_eq!(
            snapshot.items[0].available_plugs,
            vec![
                destiny_core::AvailablePlug {
                    socket_index: 2,
                    plug_hash: ItemHash::new(11)
                },
                destiny_core::AvailablePlug {
                    socket_index: 10,
                    plug_hash: ItemHash::new(12)
                }
            ]
        );
        assert!(
            snapshot.items[1..]
                .iter()
                .all(|item| item.available_plugs.is_empty())
        );
        for wrapper in [
            json!({"privacy": 2}),
            json!({"privacy": 3, "data": {}}),
            json!({"disabled": true, "data": {}}),
        ] {
            assert_validation(decode_mutated(|value| {
                value["Response"]["itemComponents"]["reusablePlugs"] = wrapper
            }));
        }
        for entries in [
            json!({"01": []}),
            json!({"256": []}),
            json!({"1": {}}),
            json!({"1": [plug(-1, true, true)]}),
            json!({"1": [{"plugItemHash": 1, "canInsert": true}]}),
        ] {
            assert_validation(decode_mutated(|value| {
                value["Response"]["itemComponents"]["reusablePlugs"]["data"] =
                    json!({"700000000000000001": {"plugs": entries}})
            }));
        }
        for count in [32, 33] {
            let sockets: serde_json::Map<String, Value> = (0..count)
                .map(|index| (index.to_string(), json!(vec![plug(1, false, false); 64])))
                .collect();
            let result = decode_mutated(|value| {
                value["Response"]["itemComponents"]["reusablePlugs"]["data"] =
                    json!({"700000000000000001": {"plugs": sockets}})
            });
            if count == 32 {
                assert!(result.is_ok());
            } else {
                assert_validation(result);
            }
        }
        let mut value = fixture_value();
        value["Response"]["itemComponents"]["reusablePlugs"]["data"] =
            json!({"700000000000000001": {"plugs": {"1": []}}});
        let text = serde_json::to_string(&value)
            .unwrap()
            .replace("\"1\":[]", "\"1\":[],\"1\":[]");
        assert_validation(decode(text.as_bytes()));
    }

    #[test]
    fn oversized_reusable_socket_is_skipped_without_losing_bounded_sockets() {
        let plug = json!({"plugItemHash": 42, "canInsert": true, "enabled": true});
        let snapshot = decode_mutated(|value| {
            value["Response"]["itemComponents"]["reusablePlugs"]["data"] = json!({"700000000000000001": {"plugs": {"1": vec![plug.clone(); 500], "2": [plug]}}});
        }).expect("oversized alternatives must not reject the profile");
        assert_eq!(
            snapshot.items[0].available_plugs,
            vec![destiny_core::AvailablePlug {
                socket_index: 2,
                plug_hash: ItemHash::new(42)
            }]
        );
        assert_validation(decode_mutated(|value| {
            let mut plugs =
                vec![json!({"plugItemHash": 42, "canInsert": true, "enabled": true}); 500];
            plugs[499]["enabled"] = json!("invalid");
            value["Response"]["itemComponents"]["reusablePlugs"]["data"] =
                json!({"700000000000000001": {"plugs": {"1": plugs}}});
        }));
    }

    #[test]
    fn reusable_plugs_component_is_required() {
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]
                .as_object_mut()
                .unwrap()
                .remove("reusablePlugs");
        }));
    }

    #[test]
    fn stats_private_data_is_sorted_and_missing_instances_are_empty() {
        let snapshot = decode_mutated(|value| {
            value["Response"]["itemComponents"]["stats"] = json!({"privacy": 2, "data": {"700000000000000001": {"stats": {"9": {"statHash": 9, "value": -10}, "1": {"statHash": 1, "value": 20}}}}});
        }).unwrap();
        assert_eq!(
            snapshot.items[0].stats,
            vec![
                destiny_core::StatSummary {
                    stat_hash: 1,
                    value: 20
                },
                destiny_core::StatSummary {
                    stat_hash: 9,
                    value: -10
                }
            ]
        );
        assert!(snapshot.items[1..].iter().all(|item| item.stats.is_empty()));
        for wrapper in [
            json!({"privacy": 2}),
            json!({"privacy": 3, "data": {}}),
            json!({"disabled": true, "data": {}}),
        ] {
            assert_validation(decode_mutated(|value| {
                value["Response"]["itemComponents"]["stats"] = wrapper
            }));
        }
        let stats: serde_json::Map<String, Value> = (0..64)
            .map(|hash| (hash.to_string(), json!({"statHash": hash, "value": 1})))
            .collect();
        assert_eq!(
            decode_mutated(|value| {
                value["Response"]["itemComponents"]["stats"] =
                    json!({"data": {"700000000000000001": {"stats": stats}}});
            })
            .unwrap()
            .items[0]
                .stats
                .len(),
            64
        );
        let stats: serde_json::Map<String, Value> = (0..65)
            .map(|hash| (hash.to_string(), json!({"statHash": hash, "value": 1})))
            .collect();
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["stats"] =
                json!({"data": {"700000000000000001": {"stats": stats}}})
        }));
    }

    #[test]
    fn duplicate_stat_keys_fail_closed() {
        let value = decode_mutated(|value| {
            value["Response"]["itemComponents"]["stats"] = json!({"data": {"700000000000000001": {"stats": {"1": {"statHash": 1, "value": 10}}}}});
        });
        assert!(value.is_ok());
        let mut value = fixture_value();
        value["Response"]["itemComponents"]["stats"] =
            json!({"data": {"700000000000000001": {"stats": {"1": {"statHash": 1, "value": 10}}}}});
        let text = serde_json::to_string(&value).unwrap().replace(
            "\"1\":{\"statHash\":1,\"value\":10}",
            "\"1\":{\"statHash\":1,\"value\":10},\"1\":{\"statHash\":1,\"value\":20}",
        );
        assert_validation(decode(text.as_bytes()));
    }

    #[test]
    fn stats_component_is_required_and_bounded() {
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]
                .as_object_mut()
                .unwrap()
                .remove("stats");
        }));
        for entry in [
            json!({"stats": {"1": {"statHash": 2, "value": 10}}}),
            json!({"stats": {"4294967296": {"statHash": 4294967296_u64, "value": 10}}}),
            json!({"stats": {"1": {"statHash": 1, "value": 2147483648_i64}}}),
            json!({"stats": {"1": {"statHash": 1, "value": 1.5}}}),
            json!({"stats": {"1": {"statHash": 1, "value": -2147483649_i64}}}),
            json!({"stats": {"-1": {"statHash": -1, "value": 10}}}),
            json!({"stats": {"01": {"statHash": 1, "value": 10}}}),
            json!({"stats": {"1": {"statHash": 1, "value": "10"}}}),
            json!({"stats": {"1": {"statHash": 1}}}),
            json!({}),
            json!({"stats": []}),
            Value::Null,
        ] {
            assert_validation(decode_mutated(|value| {
                value["Response"]["itemComponents"]["stats"] =
                    json!({"privacy": 2, "data": {"700000000000000001": entry}});
            }));
        }
    }

    #[test]
    fn plug_sets_decode_both_scopes_and_merge_flags_independently() {
        use destiny_core::{PlugSetEntry, PlugSetScope};
        let character = fixture_value()["Response"]["characters"]["data"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();
        let plug = |can_insert, enabled| json!({"plugItemHash": 42, "canInsert": can_insert, "enabled": enabled});
        let snapshot = decode_mutated(|value| {
            value["Response"]["profilePlugSets"] = json!({"privacy": 2, "data": {"plugs": {"1": [plug(false, true)], "2": [plug(true, false), plug(false, false)]}}});
            value["Response"]["characterPlugSets"] = json!({"data": {character.clone(): {"plugs": {"3": [plug(false, false)]}}}});
        }).unwrap();
        assert_eq!(
            snapshot.plug_set_entries,
            vec![
                PlugSetEntry {
                    scope: PlugSetScope::Profile,
                    plug_hash: ItemHash::new(42),
                    can_insert: true,
                    enabled: true
                },
                PlugSetEntry {
                    scope: PlugSetScope::Character(CharacterId::new(character)),
                    plug_hash: ItemHash::new(42),
                    can_insert: false,
                    enabled: false
                },
            ]
        );
        assert!(decode(FIXTURE).unwrap().plug_set_entries.is_empty());
        for wrapper in [
            json!({"privacy": 2}),
            json!({"disabled": true}),
            Value::Null,
        ] {
            assert!(
                decode_mutated(|value| {
                    value["Response"]["profilePlugSets"] = wrapper.clone();
                    value["Response"]["characterPlugSets"] = wrapper;
                })
                .unwrap()
                .plug_set_entries
                .is_empty()
            );
        }
    }

    #[test]
    fn plug_sets_reject_noncanonical_keys_and_excessive_bounds() {
        for plugs in [
            json!({"01": []}),
            json!({"4294967296": []}),
            json!({"1": vec![json!({"plugItemHash": 42, "canInsert": true, "enabled": true}); 513]}),
            Value::Object(
                (0..4097)
                    .map(|hash| (hash.to_string(), json!([])))
                    .collect(),
            ),
        ] {
            assert_validation(decode_mutated(|value| {
                value["Response"]["profilePlugSets"] = json!({"data": {"plugs": plugs}});
            }));
        }
        assert_validation(decode_mutated(|value| {
            value["Response"]["characterPlugSets"] = json!({"data": {"01": {"plugs": {}}}});
        }));
    }

    #[test]
    fn plug_set_bounds_are_inclusive_and_shared_between_scopes() {
        let character = fixture_value()["Response"]["characters"]["data"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();
        let plugs = vec![json!({"plugItemHash": 42, "canInsert": false, "enabled": false}); 512];
        let sets: serde_json::Map<String, Value> = (0..4096)
            .map(|hash| {
                (
                    hash.to_string(),
                    if hash == 0 { json!(plugs) } else { json!([]) },
                )
            })
            .collect();
        let snapshot = decode_mutated(|value| {
            value["Response"]["profilePlugSets"] = json!({"data": {"plugs": sets.clone()}});
        })
        .unwrap();
        assert_eq!(snapshot.plug_set_entries.len(), 1);
        assert!(!snapshot.plug_set_entries[0].can_insert);
        assert_validation(decode_mutated(|value| {
            value["Response"]["profilePlugSets"] = json!({"data": {"plugs": sets}});
            value["Response"]["characterPlugSets"] =
                json!({"data": {character.clone(): {"plugs": {"1": []}}}});
        }));
        for sets in [
            json!({"01": []}),
            json!({"1": vec![json!({"plugItemHash": 42, "canInsert": true, "enabled": true}); 513]}),
            json!({"1": [{"plugItemHash": -1, "canInsert": true, "enabled": true}]}),
            json!({"1": [{"plugItemHash": 42, "canInsert": "true", "enabled": true}]}),
            json!({"1": [{"plugItemHash": 42, "canInsert": true}]}),
        ] {
            assert_validation(decode_mutated(|value| {
                value["Response"]["characterPlugSets"] =
                    json!({"data": {character.clone(): {"plugs": sets}}});
            }));
        }
    }

    #[test]
    fn instance_energy_and_socket_state_are_content() {
        let baseline = decode(FIXTURE).unwrap();
        let changed = decode_mutated(|value| {
            let instance = &mut value["Response"]["itemComponents"]["instances"]["data"]["700000000000000001"];
            instance["energy"] = json!({"energyCapacity": 10, "energyUsed": 3, "energyUnused": 7, "energyType": 0, "energyTypeHash": 591714140});
            instance["gearTier"] = json!(0);
            value["Response"]["itemComponents"]["sockets"]["data"]["700000000000000001"] = json!({"sockets": [
                {"isEnabled": true, "isVisible": true},
                {"plugHash": 123, "isEnabled": false, "isVisible": true},
                {"plugHash": 456, "isEnabled": true, "isVisible": false}
            ]});
        }).unwrap();
        assert_ne!(baseline.items[0], changed.items[0]);
        let decoded = &changed.items[0];
        assert_eq!(
            decoded.energy,
            Some(destiny_core::ItemEnergy {
                energy_capacity: 10,
                energy_used: 3,
                energy_unused: 7,
                energy_type: Some(0),
                energy_type_hash: Some(591714140),
            })
        );
        assert_eq!(decoded.gear_tier, Some(0));
        assert_eq!(baseline.items[0].gear_tier, None);
        assert_eq!(
            decoded.sockets,
            vec![
                destiny_core::IndexedSocket {
                    socket_index: 0,
                    plug_hash: None,
                    enabled: true,
                    visible: true
                },
                destiny_core::IndexedSocket {
                    socket_index: 1,
                    plug_hash: Some(ItemHash::new(123)),
                    enabled: false,
                    visible: true
                },
                destiny_core::IndexedSocket {
                    socket_index: 2,
                    plug_hash: Some(ItemHash::new(456)),
                    enabled: true,
                    visible: false
                },
            ]
        );
        assert_eq!(
            decoded.plugs,
            vec![PlugSummary {
                plug_hash: ItemHash::new(456),
                enabled: true
            }]
        );
        // Energy alone must change content, even if selected plugs are identical.
        let energy_only = decode_mutated(|value| {
            value["Response"]["itemComponents"]["instances"]["data"]["700000000000000001"]["energy"] = json!({"energyCapacity": 10, "energyUsed": 3, "energyUnused": 7});
        }).unwrap();
        assert_ne!(baseline.items[0], energy_only.items[0]);

        // Real decoder output traverses the application sync seam unchanged.
        use destiny_app::{ProfileSyncService, test_support::QueuedProfileGateway};
        use destiny_core::{FreshnessPolicy, InventoryRepository, ProfileSyncReason};
        let mut gateway = QueuedProfileGateway::new();
        gateway.queue_membership(Ok(DestinyMembership::new(
            changed.membership_type,
            changed.membership_id.clone(),
        )
        .unwrap()));
        gateway.queue_inventory(Ok(changed.clone()));
        let mut service = ProfileSyncService::new(
            gateway,
            crate::sqlite::SqliteStore::open_in_memory().unwrap(),
        );
        service
            .sync(
                ProfileSyncReason::Forced,
                FreshnessPolicy { ttl_seconds: 300 },
                changed.checked_at,
            )
            .unwrap();
        let stored = service.repository().load_current().unwrap().unwrap();
        let mut fake = destiny_app::test_support::InMemoryInventoryRepository::new();
        assert_eq!(stored, fake.replace_current(changed).unwrap());
    }

    #[test]
    fn energy_and_gear_tier_validate_without_inventing_defaults() {
        for bad in [
            json!(-1),
            json!(4294967296_u64),
            json!(1.5),
            json!("1"),
            json!(true),
        ] {
            assert_validation(decode_mutated(|value| {
                value["Response"]["itemComponents"]["instances"]["data"]["700000000000000001"]["gearTier"] =
                    bad.clone();
            }));
            for field in [
                "energyCapacity",
                "energyUsed",
                "energyUnused",
                "energyType",
                "energyTypeHash",
            ] {
                assert_validation(decode_mutated(|value| {
                    let mut energy =
                        json!({"energyCapacity": 10, "energyUsed": 3, "energyUnused": 7});
                    energy[field] = bad.clone();
                    value["Response"]["itemComponents"]["instances"]["data"]["700000000000000001"]
                        ["energy"] = energy;
                }));
            }
        }
        for energy in [
            json!({}),
            json!({"energyCapacity": 10, "energyUsed": 3}),
            json!([]),
            json!(1),
        ] {
            assert_validation(decode_mutated(|value| {
                value["Response"]["itemComponents"]["instances"]["data"]["700000000000000001"]["energy"] =
                    energy;
            }));
        }
        let max = decode_mutated(|value| {
            let instance =
                &mut value["Response"]["itemComponents"]["instances"]["data"]["700000000000000001"];
            instance["gearTier"] = json!(u32::MAX);
            instance["energy"] =
                json!({"energyCapacity": u32::MAX, "energyUsed": 0, "energyUnused": u32::MAX});
        })
        .unwrap();
        assert_eq!(max.items[0].gear_tier, Some(u32::MAX));
        assert_eq!(max.items[0].energy.as_ref().unwrap().energy_type_hash, None);
        let null = decode_mutated(|value| {
            let instance =
                &mut value["Response"]["itemComponents"]["instances"]["data"]["700000000000000001"];
            instance["gearTier"] = Value::Null;
            instance["energy"] = Value::Null;
        })
        .unwrap();
        assert_eq!(null.items[0].energy, None);
        assert_eq!(null.items[0].gear_tier, None);
    }

    #[test]
    fn complete_fixture_normalizes_the_full_snapshot() {
        let snapshot = decode(FIXTURE).expect("complete fixture");

        let expected = InventorySnapshotInput {
            profile_currencies_coverage: destiny_core::ProfileCurrenciesCoverage::NotObserved,
            stacks: vec![
                destiny_core::StackObservation::new(
                    ItemHash::new(1004),
                    destiny_core::BucketHash::new(4100),
                    99,
                    destiny_core::StackContainer::ProfileInventory,
                )
                .unwrap(),
                destiny_core::StackObservation::new(
                    ItemHash::new(1005),
                    destiny_core::BucketHash::new(4100),
                    50,
                    destiny_core::StackContainer::ProfileInventory,
                )
                .unwrap(),
                destiny_core::StackObservation::new(
                    ItemHash::new(2002),
                    destiny_core::BucketHash::new(4200),
                    50,
                    destiny_core::StackContainer::CharacterInventory(character_id(
                        "900000000000000001",
                    )),
                )
                .unwrap(),
            ],
            plug_set_entries: Vec::new(),
            membership_id: MembershipId::new(MEMBERSHIP_ID),
            membership_type: MembershipType::new(MEMBERSHIP_TYPE),
            definition_version: DefinitionVersion::new(DEFINITION_VERSION),
            checked_at: Timestamp::new(CHECKED_AT),
            source: SourceMarker::Profile,
            characters: vec![
                Character {
                    id: character_id("900000000000000001"),
                    name: None,
                    class: Some("Warlock".to_owned()),
                    race: Some("Awoken".to_owned()),
                    gender: Some("Male".to_owned()),
                    light: Some(1810),
                    date_last_played: None,
                },
                Character {
                    id: character_id("900000000000000002"),
                    name: None,
                    class: Some("Hunter".to_owned()),
                    race: Some("Human".to_owned()),
                    gender: Some("Female".to_owned()),
                    light: Some(1795),
                    date_last_played: None,
                },
            ],
            items: vec![
                ItemInstance {
                    energy: None,
                    gear_tier: None,
                    sockets: Vec::new(),
                    transfer_status: None,
                    available_plugs: Vec::new(),
                    instance_id: instance_id("700000000000000001"),
                    item_hash: ItemHash::new(1001),
                    location: ItemLocation::Vault,
                    quantity: 1,
                    equipped: false,
                    locked: true,
                    power: Some(1800),
                    stats: vec![
                        destiny_core::StatSummary {
                            stat_hash: 1,
                            value: 10,
                        },
                        destiny_core::StatSummary {
                            stat_hash: 9,
                            value: 20,
                        },
                    ],
                    plugs: Vec::new(),
                    perks: Vec::new(),
                },
                ItemInstance {
                    energy: None,
                    gear_tier: None,
                    sockets: Vec::new(),
                    transfer_status: None,
                    available_plugs: Vec::new(),
                    instance_id: instance_id("700000000000000003"),
                    item_hash: ItemHash::new(1003),
                    location: ItemLocation::other("bungie-profile-location:3"),
                    quantity: 5,
                    equipped: false,
                    locked: false,
                    power: None,
                    stats: Vec::new(),
                    plugs: Vec::new(),
                    perks: Vec::new(),
                },
                ItemInstance {
                    energy: None,
                    gear_tier: None,
                    sockets: Vec::new(),
                    transfer_status: None,
                    available_plugs: Vec::new(),
                    instance_id: instance_id("700000000000000004"),
                    item_hash: ItemHash::new(2001),
                    stats: Vec::new(),
                    plugs: Vec::new(),
                    location: ItemLocation::Character(character_id("900000000000000001")),
                    quantity: 1,
                    equipped: false,
                    locked: false,
                    power: None,
                    perks: Vec::new(),
                },
                ItemInstance {
                    energy: None,
                    gear_tier: None,
                    sockets: Vec::new(),
                    transfer_status: None,
                    available_plugs: Vec::new(),
                    instance_id: instance_id("700000000000000005"),
                    item_hash: ItemHash::new(2003),
                    stats: Vec::new(),
                    plugs: Vec::new(),
                    location: ItemLocation::Character(character_id("900000000000000002")),
                    quantity: 3,
                    equipped: false,
                    locked: false,
                    power: None,
                    perks: Vec::new(),
                },
                ItemInstance {
                    energy: None,
                    gear_tier: None,
                    sockets: [
                        (Some(6001), true, true),
                        (Some(6002), false, true),
                        (None, true, false),
                        (None, true, false),
                        (Some(6003), true, false),
                        (Some(6001), true, true),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(
                        |(index, (hash, enabled, visible))| destiny_core::IndexedSocket {
                            socket_index: index as u8,
                            plug_hash: hash.map(ItemHash::new),
                            enabled,
                            visible,
                        },
                    )
                    .collect(),
                    transfer_status: None,
                    available_plugs: Vec::new(),
                    instance_id: instance_id("700000000000000006"),
                    item_hash: ItemHash::new(3001),
                    location: ItemLocation::Character(character_id("900000000000000001")),
                    quantity: 1,
                    equipped: true,
                    locked: true,
                    power: Some(1810),
                    stats: Vec::new(),
                    plugs: vec![
                        PlugSummary {
                            plug_hash: ItemHash::new(6001),
                            enabled: true,
                        },
                        PlugSummary {
                            plug_hash: ItemHash::new(6003),
                            enabled: true,
                        },
                        PlugSummary {
                            plug_hash: ItemHash::new(6001),
                            enabled: true,
                        },
                    ],
                    perks: vec![
                        PerkSummary {
                            perk_hash: PerkHash::new(5001),
                            active: true,
                            visible: true,
                        },
                        PerkSummary {
                            perk_hash: PerkHash::new(5002),
                            active: false,
                            visible: false,
                        },
                        PerkSummary {
                            perk_hash: PerkHash::new(5001),
                            active: false,
                            visible: true,
                        },
                    ],
                },
                ItemInstance {
                    energy: None,
                    gear_tier: None,
                    sockets: Vec::new(),
                    transfer_status: None,
                    available_plugs: Vec::new(),
                    instance_id: instance_id("700000000000000007"),
                    item_hash: ItemHash::new(3002),
                    stats: Vec::new(),
                    plugs: Vec::new(),
                    location: ItemLocation::Character(character_id("900000000000000002")),
                    quantity: 1,
                    equipped: true,
                    locked: false,
                    power: None,
                    perks: Vec::new(),
                },
            ],
        };

        assert_eq!(snapshot, expected);
        assert!(snapshot.is_structurally_valid());

        // The fixture intentionally contains instance-less and Postmaster items
        // that must not appear in the projection.
        let retained: Vec<&str> = snapshot
            .items
            .iter()
            .map(|item| item.instance_id.as_str())
            .collect();
        assert_eq!(
            retained,
            vec![
                "700000000000000001",
                "700000000000000003",
                "700000000000000004",
                "700000000000000005",
                "700000000000000006",
                "700000000000000007",
            ]
        );
    }

    #[test]
    fn sockets_preserve_enabled_plug_order_and_duplicates_regardless_of_visibility() {
        let snapshot = decode(FIXTURE).expect("socket fixture");
        assert_eq!(
            item(&snapshot, "700000000000000006").plugs,
            [
                PlugSummary {
                    plug_hash: ItemHash::new(6001),
                    enabled: true
                },
                PlugSummary {
                    plug_hash: ItemHash::new(6003),
                    enabled: true
                },
                PlugSummary {
                    plug_hash: ItemHash::new(6001),
                    enabled: true
                }
            ]
        );
        assert!(item(&snapshot, "700000000000000001").plugs.is_empty());
    }

    #[test]
    fn sockets_missing_entry_and_empty_list_decode_as_empty() {
        for data in [json!({}), json!({"700000000000000006": {"sockets": []}})] {
            let snapshot = decode_mutated(|value| {
                value["Response"]["itemComponents"]["sockets"]["data"] = data;
            })
            .expect("missing or empty entry");
            assert!(item(&snapshot, "700000000000000006").plugs.is_empty());
        }
    }

    #[test]
    fn sockets_owner_private_with_data_is_accepted() {
        let snapshot = decode_mutated(|value| {
            value["Response"]["itemComponents"]["sockets"]["privacy"] = json!(2);
        })
        .expect("owner private sockets");
        assert_eq!(snapshot, decode(FIXTURE).unwrap());
    }

    #[test]
    fn sockets_component_is_required_and_validated() {
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]
                .as_object_mut()
                .unwrap()
                .remove("sockets");
        }));
        for wrapper in [
            json!({"privacy": 2}),
            json!({"privacy": 3, "data": {}}),
            json!({"disabled": true, "data": {}}),
            json!({"data": null}),
        ] {
            assert_validation(decode_mutated(|value| {
                value["Response"]["itemComponents"]["sockets"] = wrapper;
            }));
        }
    }

    #[test]
    fn sockets_cap_is_enforced_before_filtering_empty_or_disabled_plugs() {
        for socket in [
            json!({"isEnabled": true, "isVisible": false}),
            json!({"plugHash": 6001, "isEnabled": false, "isVisible": true}),
        ] {
            decode_mutated(|value| {
                value["Response"]["itemComponents"]["sockets"]["data"]["700000000000000006"]["sockets"] =
                    json!(vec![socket.clone(); 32]);
            }).expect("32 sockets are accepted");
            assert_validation(decode_mutated(|value| {
                value["Response"]["itemComponents"]["sockets"]["data"]["700000000000000006"]["sockets"] =
                    json!(vec![socket; 33]);
            }));
        }
    }

    #[test]
    fn malformed_sockets_fail_closed() {
        for socket in [
            json!({"plugHash": 4294967296u64, "isEnabled": true, "isVisible": true}),
            json!({"plugHash": -1, "isEnabled": true, "isVisible": true}),
            json!({"plugHash": "6001", "isEnabled": false, "isVisible": true}),
            json!({"plugHash": 6001, "isEnabled": null, "isVisible": true}),
            json!({"plugHash": 6001, "isEnabled": true}),
            json!({"plugHash": null, "isEnabled": true, "isVisible": 1}),
            Value::Null,
        ] {
            assert_validation(decode_mutated(|value| {
                value["Response"]["itemComponents"]["sockets"]["data"]["700000000000000006"]["sockets"] =
                    json!([socket]);
            }));
        }
        for entry in [Value::Null, json!({}), json!({"sockets": {}})] {
            assert_validation(decode_mutated(|value| {
                value["Response"]["itemComponents"]["sockets"]["data"]["700000000000000006"] =
                    entry;
            }));
        }
    }

    #[test]
    fn error_code_and_envelope_failures_are_provider_validation() {
        assert_validation(decode("{not json".as_bytes()));
        assert_validation(decode("[]".as_bytes()));
        assert_validation(decode("{}".as_bytes()));
        assert_validation(decode_mutated(|value| {
            value["ErrorCode"] = json!(0);
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"] = Value::Null;
        }));
    }

    #[test]
    fn missing_or_private_components_are_rejected() {
        assert_validation(decode_mutated(|value| {
            value["Response"]
                .as_object_mut()
                .expect("response")
                .remove("profileInventory");
        }));
        assert_validation(decode_mutated(|value| {
            let characters = &mut value["Response"]["characters"];
            characters["privacy"] = json!(2);
            characters
                .as_object_mut()
                .expect("characters")
                .remove("data");
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["characterEquipment"]["privacy"] = json!(3);
        }));
        assert_validation(decode_mutated(|value| {
            let perks = &mut value["Response"]["itemComponents"]["perks"];
            perks["privacy"] = json!(2);
            perks.as_object_mut().expect("perks").remove("data");
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["characters"]["disabled"] = json!(true);
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["characters"]["disabled"] = json!(1);
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["instances"]["data"] = json!(null);
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]
                .as_object_mut()
                .expect("item components")
                .remove("perks");
        }));
    }

    #[test]
    fn absent_privacy_and_public_privacy_are_accepted() {
        decode_mutated(|value| {
            value["Response"]["characters"]
                .as_object_mut()
                .expect("characters")
                .remove("privacy");
        })
        .expect("absent privacy is accepted");

        decode_mutated(|value| {
            value["Response"]["characters"]["privacy"] = json!(0);
        })
        .expect("privacy 0 is accepted");

        decode_mutated(|value| {
            value["Response"]["characters"]["privacy"] = json!(1);
        })
        .expect("privacy 1 is accepted");
    }

    #[test]
    fn owner_private_components_with_data_are_accepted() {
        // Bungie marks owner-only components (vault, character inventories,
        // equipment, item components) as privacy 2 and still includes `data`
        // when the request is authorized as the owner.
        decode_mutated(|value| {
            value["Response"]["profileInventory"]["privacy"] = json!(2);
            value["Response"]["characterInventories"]["privacy"] = json!(2);
            value["Response"]["characterEquipment"]["privacy"] = json!(2);
            value["Response"]["itemComponents"]["instances"]["privacy"] = json!(2);
            value["Response"]["itemComponents"]["perks"]["privacy"] = json!(2);
        })
        .expect("owner-private components with data are accepted");
    }

    #[test]
    fn character_identifier_and_component_consistency_is_enforced() {
        assert_validation(decode_mutated(|value| {
            value["Response"]["characters"]["data"]["900000000000000001"]["characterId"] =
                json!("900000000000000099");
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["characters"]["data"]["900000000000000001"]["characterId"] = json!(5);
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["characterInventories"]["data"]
                .as_object_mut()
                .expect("inventory")
                .remove("900000000000000002");
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["characterEquipment"]["data"]
                .as_object_mut()
                .expect("equipment")
                .remove("900000000000000002");
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["characterInventories"]["data"]["900000000000000099"] =
                json!({ "items": [] });
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["characters"]["data"]["900000000000000001"]["classType"] =
                json!("Warlock");
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["characters"]["data"]["900000000000000001"]["light"] = json!(70000);
        }));
    }

    #[test]
    fn item_identity_and_equipment_conflicts_are_rejected() {
        assert_validation(decode_mutated(|value| {
            value["Response"]["characterEquipment"]["data"]["900000000000000001"]["items"][0]["itemInstanceId"] =
                json!("700000000000000001");
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["instances"]["data"]["700000000000000006"]["isEquipped"] =
                json!(false);
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["instances"]["data"]["700000000000000004"]["isEquipped"] =
                json!(true);
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["instances"]["data"]
                .as_object_mut()
                .expect("instances")
                .remove("700000000000000001");
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["instances"]["data"]["700000000000000001"]["isEquipped"] =
                json!("false");
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["characterInventories"]["data"]["900000000000000001"]["items"][0]["itemInstanceId"] =
                json!("not-a-decimal-id");
        }));
    }

    #[test]
    fn missing_required_item_fields_are_rejected() {
        assert_validation(decode_mutated(|value| {
            value["Response"]["profileInventory"]["data"]["items"][0]["itemHash"] = Value::Null;
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["profileInventory"]["data"]["items"][0]["quantity"] = json!("1");
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["profileInventory"]["data"]["items"][0]["state"] =
                json!(4294967296u64);
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["profileInventory"]["data"]["items"][0]["location"] = json!("2");
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["profileInventory"]["data"]["items"] = json!(null);
        }));
    }

    #[test]
    fn out_of_range_integer_power_is_unknown_power() {
        // Live profiles carry items whose primary stat is negative (observed
        // -10); an integer outside the power range is not a malformed response.
        for raw in [json!(-10), json!(70000)] {
            let snapshot =
                decode_mutated(|value| {
                    value["Response"]["itemComponents"]["instances"]["data"]["700000000000000001"]
                        ["primaryStat"]["value"] = raw;
                })
                .expect("an out-of-range integer power is accepted");
            let item = snapshot
                .items
                .iter()
                .find(|item| item.instance_id == instance_id("700000000000000001"))
                .expect("the item is decoded");
            assert_eq!(item.power, None);
        }
    }

    #[test]
    fn malformed_power_and_perks_are_rejected() {
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["instances"]["data"]["700000000000000001"]["primaryStat"]
                ["value"] = json!(1800.5);
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["instances"]["data"]["700000000000000001"]["primaryStat"]
                ["value"] = json!("1800");
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["instances"]["data"]["700000000000000001"]["primaryStat"] =
                json!(1800);
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["perks"]["data"]["700000000000000006"]["perks"]
                [0]["perkHash"] = json!(4294967296u64);
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["perks"]["data"]["700000000000000006"]["perks"]
                [0]["isActive"] = json!(1);
        }));
        // Official `visible` is a boolean; numeric, string, and null forms fail.
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["perks"]["data"]["700000000000000006"]["perks"]
                [0]["visible"] = json!(0);
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["perks"]["data"]["700000000000000006"]["perks"]
                [0]["visible"] = json!(1);
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["perks"]["data"]["700000000000000006"]["perks"]
                [0]["visible"] = json!("true");
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["perks"]["data"]["700000000000000006"]["perks"]
                [0]["visible"] = Value::Null;
        }));
        assert_validation(decode_mutated(|value| {
            value["Response"]["itemComponents"]["perks"]["data"]["700000000000000006"]["perks"] =
                json!({});
        }));
    }

    #[test]
    fn failures_never_leak_input_sentinels() {
        let sentinel = "sentinel-provider-secret";
        let error = decode_mutated(|value| {
            value["Response"]["characters"]["data"]["900000000000000001"]["classType"] =
                json!(sentinel);
        })
        .expect_err("malformed class");
        assert_eq!(error, AppError::new(ErrorCategory::ProviderValidation));
        assert!(!error.to_string().contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));
    }

    fn set_character_field(field: &str, to: Value) -> impl FnOnce(&mut Value) {
        let field = field.to_owned();
        move |value: &mut Value| {
            value
                .pointer_mut("/Response/characters/data/900000000000000001")
                .expect("character")
                .as_object_mut()
                .expect("character object")
                .insert(field.clone(), to.clone());
        }
    }

    fn remove_character_field(field: &str) -> impl FnOnce(&mut Value) {
        let field = field.to_owned();
        move |value: &mut Value| {
            value
                .pointer_mut("/Response/characters/data/900000000000000001")
                .expect("character")
                .as_object_mut()
                .expect("character object")
                .remove(field.as_str());
        }
    }

    fn set_profile_item_field(index: usize, field: &str, to: Value) -> impl FnOnce(&mut Value) {
        let field = field.to_owned();
        move |value: &mut Value| {
            value
                .pointer_mut("/Response/profileInventory/data/items")
                .expect("profile items")
                .as_array_mut()
                .expect("profile item array")[index]
                .as_object_mut()
                .expect("profile item")
                .insert(field.clone(), to.clone());
        }
    }

    fn item<'a>(snapshot: &'a InventorySnapshotInput, id: &str) -> &'a ItemInstance {
        snapshot
            .items
            .iter()
            .find(|item| item.instance_id.as_str() == id)
            .expect("retained item")
    }

    #[test]
    fn known_and_unknown_character_enum_codes_map_deterministically() {
        let snapshot = decode_mutated(|value| {
            value["Response"]["characters"]["data"]["900000000000000001"]["classType"] = json!(9);
        })
        .expect("unknown class");
        assert_eq!(snapshot.characters[0].class.as_deref(), Some("Unknown(9)"));

        let snapshot = decode_mutated(|value| {
            value["Response"]["characters"]["data"]["900000000000000001"]["raceType"] = json!(3);
        })
        .expect("unknown race");
        assert_eq!(snapshot.characters[0].race.as_deref(), Some("Unknown(3)"));

        let snapshot = decode_mutated(|value| {
            value["Response"]["characters"]["data"]["900000000000000001"]["genderType"] = json!(2);
        })
        .expect("unknown gender");
        assert_eq!(snapshot.characters[0].gender.as_deref(), Some("Unknown(2)"));

        // The largest valid u32 code is preserved rather than rejected.
        let snapshot = decode_mutated(set_character_field("classType", json!(4294967295u64)))
            .expect("boundary enum code");
        assert_eq!(
            snapshot.characters[0].class.as_deref(),
            Some("Unknown(4294967295)")
        );
    }

    #[test]
    fn character_enums_are_required_non_null_u32() {
        for field in ["classType", "raceType", "genderType"] {
            assert_validation(decode_mutated(remove_character_field(field)));
            assert_validation(decode_mutated(set_character_field(field, Value::Null)));
            assert_validation(decode_mutated(set_character_field(field, json!(1.5))));
            assert_validation(decode_mutated(set_character_field(
                field,
                json!(4294967296u64),
            )));
            assert_validation(decode_mutated(set_character_field(field, json!(i64::MIN))));
            assert_validation(decode_mutated(set_character_field(field, json!("2"))));
        }
    }

    #[test]
    fn profile_locations_preserve_unknown_values_and_reject_unowned_postmaster() {
        // The synthetic fixture carries Vendor (3), a preserved unknown location.
        let snapshot = decode(FIXTURE).expect("fixture");
        assert_eq!(
            item(&snapshot, "700000000000000003").location,
            ItemLocation::other("bungie-profile-location:3")
        );
        assert!(
            snapshot
                .items
                .iter()
                .all(|item| item.instance_id.as_str() != "700000000000000002")
        );

        // Unknown (0) and Inventory (1) profile locations are preserved as Other.
        for location in [0u64, 1] {
            let snapshot = decode_mutated(set_profile_item_field(0, "location", json!(location)))
                .expect("preserved location");
            assert_eq!(
                item(&snapshot, "700000000000000001").location,
                ItemLocation::other(format!("bungie-profile-location:{location}"))
            );
        }

        // Neither gear nor stacks in profile scope have a validated owner.
        for index in [0, 2] {
            assert_validation(decode_mutated(set_profile_item_field(
                index,
                "location",
                json!(4),
            )));
        }
    }

    #[test]
    fn decimal_identifiers_accept_string_and_exact_u64_forms() {
        // A u64 item instance id canonicalizes and resolves its instance record.
        let snapshot = decode_mutated(|value| {
            value["Response"]["profileInventory"]["data"]["items"][0]["itemInstanceId"] =
                json!(700000000000000001u64);
        })
        .expect("u64 instance id");
        assert!(
            snapshot
                .items
                .iter()
                .any(|item| { item.instance_id.as_str() == "700000000000000001" })
        );

        // A u64 character id canonicalizes and matches the map key.
        let snapshot = decode_mutated(set_character_field(
            "characterId",
            json!(900000000000000001u64),
        ))
        .expect("u64 character id");
        assert_eq!(snapshot.characters[0].id.as_str(), "900000000000000001");

        // A mismatched u64 character id is still rejected.
        assert_validation(decode_mutated(set_character_field(
            "characterId",
            json!(900000000000000099u64),
        )));

        // Floats, negatives, booleans, and containers are never accepted.
        for id in [
            json!(900000000000000001.0f64),
            json!(i64::MIN),
            json!(true),
            json!([1]),
            json!(null),
        ] {
            assert_validation(decode_mutated(set_character_field("characterId", id)));
        }
    }

    #[test]
    fn decimal_identifiers_are_bounded_to_128_digits() {
        let at_limit = "9".repeat(128);
        let item_id = at_limit.clone();
        let instance_key = at_limit.clone();
        let snapshot = decode_mutated(move |value| {
            value["Response"]["profileInventory"]["data"]["items"][0]["itemInstanceId"] =
                json!(item_id);
            value["Response"]["itemComponents"]["instances"]["data"][instance_key.as_str()] =
                json!({ "isEquipped": false });
        })
        .expect("128-digit identifier is accepted");
        assert!(
            snapshot
                .items
                .iter()
                .any(|item| item.instance_id.as_str() == at_limit)
        );

        for oversize in ["9".repeat(129), String::new(), "12x".to_owned()] {
            let item_id = oversize.clone();
            let instance_key = oversize.clone();
            assert_validation(decode_mutated(move |value| {
                value["Response"]["profileInventory"]["data"]["items"][0]["itemInstanceId"] =
                    json!(item_id);
                value["Response"]["itemComponents"]["instances"]["data"][instance_key.as_str()] =
                    json!({ "isEquipped": false });
            }));
        }
    }

    #[test]
    fn item_hash_and_profile_location_enforce_u32_bounds() {
        let snapshot = decode_mutated(set_profile_item_field(0, "itemHash", json!(4294967295u64)))
            .expect("max u32 item hash");
        assert_eq!(
            item(&snapshot, "700000000000000001").item_hash.value(),
            i64::from(u32::MAX)
        );
        assert_validation(decode_mutated(set_profile_item_field(
            0,
            "itemHash",
            json!(4294967296u64),
        )));
        assert_validation(decode_mutated(set_profile_item_field(
            0,
            "itemHash",
            json!(i64::MIN),
        )));
        assert_validation(decode_mutated(set_profile_item_field(
            0,
            "itemHash",
            json!(1.5),
        )));

        let snapshot = decode_mutated(set_profile_item_field(0, "location", json!(4294967295u64)))
            .expect("max u32 location");
        assert_eq!(
            item(&snapshot, "700000000000000001").location,
            ItemLocation::other("bungie-profile-location:4294967295")
        );
        assert_validation(decode_mutated(set_profile_item_field(
            0,
            "location",
            json!(4294967296u64),
        )));
        assert_validation(decode_mutated(set_profile_item_field(
            0,
            "location",
            json!(i64::MIN),
        )));
    }

    #[test]
    fn decimal_identifiers_must_be_canonical() {
        // The validator accepts the literal zero and canonical non-zero values.
        assert!(super::validate_decimal_id("0").is_ok());
        assert!(super::validate_decimal_id("10").is_ok());
        let at_limit = "9".repeat(128);
        assert!(super::validate_decimal_id(at_limit.as_str()).is_ok());

        for invalid in [
            "",
            "00",
            "01",
            "007",
            "0x1",
            "1a",
            "-1",
            " 1",
            "\u{661}\u{662}\u{663}",
        ] {
            assert_eq!(
                super::validate_decimal_id(invalid),
                Err(AppError::new(ErrorCategory::ProviderValidation)),
                "identifier {invalid:?} must be rejected"
            );
        }
        let over_limit = "9".repeat(129);
        assert_eq!(
            super::validate_decimal_id(over_limit.as_str()),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );

        // End to end: a literal zero instance id is accepted when its instance
        // record exists, while a leading-zero instance id is rejected.
        let snapshot = decode_mutated(|value| {
            value["Response"]["profileInventory"]["data"]["items"][0]["itemInstanceId"] =
                json!("0");
            value["Response"]["itemComponents"]["instances"]["data"]["0"] =
                json!({ "isEquipped": false });
        })
        .expect("literal zero id");
        assert!(
            snapshot
                .items
                .iter()
                .any(|item| item.instance_id.as_str() == "0")
        );

        for leading_zero in ["00", "01", "007"] {
            let id = leading_zero.to_owned();
            let instance_key = leading_zero.to_owned();
            assert_validation(decode_mutated(move |value| {
                value["Response"]["profileInventory"]["data"]["items"][0]["itemInstanceId"] =
                    json!(id);
                value["Response"]["itemComponents"]["instances"]["data"][instance_key.as_str()] =
                    json!({ "isEquipped": false });
            }));
        }
    }

    #[test]
    fn character_and_item_order_is_numeric_decimal_not_lexicographic() {
        // Character ids "9" and "10" and instance ids "20" and "100" are
        // canonical but mixed length. Lexicographic or input order would emit
        // "10" before "9" and "100" before "20"; numeric decimal order must
        // emit "9", "10" and "20", "100".
        let payload = json!({
            "ErrorCode": 1,
            "Response": {
                "characters": {
                    "data": {
                        "10": { "characterId": "10", "classType": 0, "raceType": 0, "genderType": 0 },
                        "9": { "characterId": "9", "classType": 0, "raceType": 0, "genderType": 0 }
                    }
                },
                "profileInventory": { "data": { "items": [] } },
                "characterInventories": {
                    "data": {
                        "10": { "items": [ { "itemHash": 2, "itemInstanceId": "100", "quantity": 1, "location": 1, "state": 0 } ] },
                        "9": { "items": [ { "itemHash": 1, "itemInstanceId": "20", "quantity": 1, "location": 1, "state": 0 } ] }
                    }
                },
                "characterEquipment": {
                    "data": {
                        "10": { "items": [] },
                        "9": { "items": [] }
                    }
                },
                "itemComponents": {
                    "instances": { "data": { "20": { "isEquipped": false }, "100": { "isEquipped": false } } },
                    "perks": { "data": {} },
                    "sockets": { "data": {} },
                    "stats": { "data": {} },
                    "reusablePlugs": { "data": {} }
                }
            }
        });
        let bytes = serde_json::to_vec(&payload).expect("payload serializes");
        let snapshot = decode(&bytes).expect("valid reordered payload");

        let character_ids: Vec<&str> = snapshot
            .characters
            .iter()
            .map(|character| character.id.as_str())
            .collect();
        assert_eq!(character_ids, vec!["9", "10"]);

        let instance_ids: Vec<&str> = snapshot
            .items
            .iter()
            .map(|item| item.instance_id.as_str())
            .collect();
        assert_eq!(instance_ids, vec!["20", "100"]);
        assert_eq!(
            snapshot.items[0].location,
            ItemLocation::Character(character_id("9"))
        );
        assert_eq!(
            snapshot.items[1].location,
            ItemLocation::Character(character_id("10"))
        );
    }
}
