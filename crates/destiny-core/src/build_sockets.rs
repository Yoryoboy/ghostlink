//! Pure DIM socket-override planning for subclasses and artifacts.
//! DIM `loadout-apply.ts:272-299,980-1260` (override gathering and per-category
//! assignment) and `advanced-write-actions.ts:41-103` (`canInsertForFree`), commit 29c122e.
//! Assignments are computed from the current sockets and applied in order, one request
//! per changed socket; a plug that is already in its socket needs no request.
use crate::{armor_catalog::*, build_analysis::ResolvedBuildItem, *};
use std::collections::BTreeMap;

const ASPECT_CATEGORIES: [u32; 4] = [2047681910, 2140934067, 764703411, 3400923910];
const FRAGMENT_CATEGORIES: [u32; 4] = [271461480, 1313488945, 193371309, 2819965312];
/// DIM `subclassAbilitySocketCategoryHashes` (`socket-utils.ts:292-296`).
const ABILITY_CATEGORIES: [u32; 3] = [309722977, 3218807805, 457473665];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SocketSkipReason {
    Unresolved,
    MissingDefinition,
    ClassMismatch,
    NotOnCharacter,
    SocketStateUnavailable,
    PlugDefinitionUnavailable,
    /// DIM would read an empty plug the projection cannot establish.
    EmptyPlugUnknown,
    AbilityDefaultUnknown,
    FragmentCapacityUnknown,
    /// `canInsertForFree` is false: DIM would need an Advanced Write Action.
    NotFree,
    InsertionCostUnavailable,
}
impl SocketSkipReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unresolved => "unresolved",
            Self::MissingDefinition => "missing_definition",
            Self::ClassMismatch => "class_mismatch",
            Self::NotOnCharacter => "not_on_target_character",
            Self::SocketStateUnavailable => "socket_state_unavailable",
            Self::PlugDefinitionUnavailable => "plug_definition_unavailable",
            Self::EmptyPlugUnknown => "empty_plug_unknown",
            Self::AbilityDefaultUnknown => "ability_default_unknown",
            Self::FragmentCapacityUnknown => "fragment_capacity_unknown",
            Self::NotFree => "not_free",
            Self::InsertionCostUnavailable => "insertion_cost_unavailable",
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SocketInsert {
    pub spec_index: usize,
    pub item: ItemInstanceId,
    pub socket_index: u8,
    pub plug: ItemHash,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SocketSkip {
    pub spec_index: usize,
    /// The build's override index (DIM reports progress by it), when the skip is per socket.
    pub socket_index: Option<u16>,
    pub reason: SocketSkipReason,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SocketPlan {
    pub inserts: Vec<SocketInsert>,
    pub skipped: Vec<SocketSkip>,
    /// Requested overrides whose plug is already in place, by override index.
    pub already_there: Vec<(usize, u16)>,
}

pub(crate) struct Socket<'a> {
    pub(crate) index: u8,
    pub(crate) plugged: Option<u32>,
    pub(crate) empty: Option<u32>,
    /// Plug-set members; DIM `socket.plugSet.plugs`.
    pub(crate) set_plugs: Vec<u32>,
    pub(crate) entry: &'a DefinitionSocketEntry,
}
/// DIM `hasInsertionCost` (`advanced-write-actions.ts:50-59`).
fn has_cost(
    defs: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    hash: u32,
) -> Result<bool, SocketSkipReason> {
    let plug = defs
        .iter()
        .find(|d| d.hash.value() == i64::from(hash))
        .ok_or(SocketSkipReason::PlugDefinitionUnavailable)?;
    let Some(set) = plug
        .armor
        .insertion_material_requirement_hash
        .filter(|h| *h != 0)
    else {
        return Ok(false);
    };
    catalogs
        .iter()
        .find_map(|c| match c {
            ArmorCatalogRecord::MaterialRequirementSet(s) if s.hash == set => Some(s),
            _ => None,
        })
        .map(|s| s.materials.iter().any(|m| m.count != 0))
        .ok_or(SocketSkipReason::InsertionCostUnavailable)
}
/// DIM `canInsertForFree` (`advanced-write-actions.ts:61-103`). The core-settings
/// protected-plug and blocked-socket-type lists are not available here; Bungie
/// remains the final validator for those.
pub(crate) fn can_insert_free(
    defs: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    entry: &DefinitionSocketEntry,
    plugged: Option<u32>,
    plug: u32,
) -> Result<(), SocketSkipReason> {
    let reusable = !entry.reusable_plug_items.is_empty()
        || entry.reusable_plug_set_hash.is_some_and(|h| h != 0)
        || entry.randomized_plug_set_hash.is_some_and(|h| h != 0);
    let current_cost = plugged.map(|p| has_cost(defs, catalogs, p)).transpose()?;
    if reusable && !has_cost(defs, catalogs, plug)? && current_cost != Some(true) {
        Ok(())
    } else {
        Err(SocketSkipReason::NotFree)
    }
}

/// DIM `Assignment`: unrequested ones only clear a socket and are never reported.
struct Assignment {
    socket: u8,
    plug: u32,
    requested: bool,
    reported: u16,
}
struct Needed {
    hash: u32,
    /// Where the plug is meant to go; artifacts remap it to avoid extra insertions.
    loadout: u32,
    original: u16,
}
struct Item<'a> {
    spec: usize,
    id: ItemInstanceId,
    definition: &'a ItemDefinition,
    sockets: Vec<Socket<'a>>,
    overrides: BTreeMap<u32, u32>,
    defs: &'a [ItemDefinition],
    catalogs: &'a [ArmorCatalogRecord],
    plan: &'a mut SocketPlan,
}

impl Item<'_> {
    fn def(&self, hash: u32) -> Option<&ItemDefinition> {
        self.defs.iter().find(|d| d.hash.value() == i64::from(hash))
    }
    fn socket(&self, index: u32) -> Option<&Socket<'_>> {
        self.sockets.iter().find(|s| u32::from(s.index) == index)
    }
    fn skip(&mut self, reported: u16, reason: SocketSkipReason) {
        self.plan.skipped.push(SocketSkip {
            spec_index: self.spec,
            socket_index: Some(reported),
            reason,
        });
    }
    /// DIM `canInsertForFree` (`advanced-write-actions.ts:61-103`).
    fn free(&self, socket: &Socket<'_>, plug: u32) -> Result<(), SocketSkipReason> {
        can_insert_free(self.defs, self.catalogs, socket.entry, socket.plugged, plug)
    }
    /// DIM `equipModsToItem` for one assignment (`loadout-apply.ts:1303-1366`).
    fn assign(&mut self, a: Assignment) {
        let Some(at) = self.sockets.iter().position(|s| s.index == a.socket) else {
            return;
        };
        if self.sockets[at].plugged == Some(a.plug) {
            if a.requested {
                self.plan.already_there.push((self.spec, a.reported));
            }
            return;
        }
        match self.free(&self.sockets[at], a.plug) {
            Ok(()) => {
                self.sockets[at].plugged = Some(a.plug);
                self.plan.inserts.push(SocketInsert {
                    spec_index: self.spec,
                    item: self.id.clone(),
                    socket_index: a.socket,
                    plug: ItemHash::new(i64::from(a.plug)),
                });
            }
            Err(reason) if a.requested => self.skip(a.reported, reason),
            Err(_) => {}
        }
    }
    /// Clears a socket to its default; DIM `emptyPlugItemHash!`.
    fn clear(&mut self, socket: u8) {
        if let Some(plug) = self.socket(socket.into()).and_then(|s| s.empty) {
            self.assign(Assignment {
                socket,
                plug,
                requested: false,
                reported: socket.into(),
            });
        }
    }
    fn keep_or_insert(&mut self, socket: u8, plug: u32, reported: u16) {
        self.assign(Assignment {
            socket,
            plug,
            requested: true,
            reported,
        });
    }
    /// DIM `handleShuffledSockets` (`loadout-apply.ts:1142-1192`): aspects and fragments
    /// are interchangeable between their sockets, so plugs already present stay.
    fn shuffled(&mut self, indexes: &[u32]) {
        let mut needed: Vec<Needed> = indexes
            .iter()
            .filter_map(|i| {
                self.overrides.get(i).map(|hash| Needed {
                    hash: *hash,
                    loadout: *i,
                    original: *i as u16,
                })
            })
            .collect();
        // If the loadout doesn't specify any, DIM leaves them as they are.
        if needed.is_empty() {
            return;
        }
        let present: Vec<(u8, Option<u32>)> = indexes
            .iter()
            .filter_map(|i| self.socket(*i))
            .map(|s| (s.index, s.plugged))
            .collect();
        let mut excess = vec![];
        for (socket, plugged) in present {
            let Some(plugged) = plugged else { continue };
            match needed.iter().position(|n| n.hash == plugged) {
                Some(at) => {
                    let kept = needed.remove(at);
                    self.keep_or_insert(socket, plugged, kept.original);
                }
                None => excess.push(socket),
            }
        }
        for socket in excess {
            match needed.pop() {
                Some(n) => self.keep_or_insert(socket, n.hash, n.original),
                None => self.clear(socket),
            }
        }
    }
    /// DIM `getLoadoutSubclassFragmentCapacity(.., true)` (`loadout-utils.ts:745-771`):
    /// the loadout's own aspects when it has any, otherwise the plugged ones.
    fn fragment_capacity(&self) -> Option<i32> {
        let aspects = self
            .definition
            .armor
            .socket_categories
            .iter()
            .find(|c| ASPECT_CATEGORIES.contains(&c.socket_category_hash))
            .map(|c| c.socket_indexes.as_slice())
            .unwrap_or_default();
        let chosen: Vec<&ItemDefinition> = aspects
            .iter()
            .filter_map(|i| self.overrides.get(i))
            .filter_map(|hash| self.def(*hash))
            .collect();
        if !chosen.is_empty() {
            return Some(
                chosen
                    .iter()
                    .map(|d| d.armor.aspect_capacity.unwrap_or(0))
                    .sum(),
            );
        }
        let sockets: Vec<IndexedSocket> = self
            .sockets
            .iter()
            .map(|s| IndexedSocket {
                plug_objectives: None,
                reusable_plugs: None,
                socket_index: s.index,
                plug_hash: s.plugged.map(|p| ItemHash::new(i64::from(p))),
                enabled: true,
                visible: true,
            })
            .collect();
        crate::armor_model::subclass_fragment_capacity(
            &self.definition.armor,
            Some(&sockets),
            |h| self.def(h).map(|d| &d.armor),
        )
    }
    /// DIM patches missing ability/super sockets with their default choice
    /// (`loadout-apply.ts:283-299`, `socket-utils.ts:244-250`).
    fn patch_ability_defaults(&mut self) {
        let indexes: Vec<u32> = self
            .definition
            .armor
            .socket_categories
            .iter()
            .filter(|c| ABILITY_CATEGORIES.contains(&c.socket_category_hash))
            .flat_map(|c| c.socket_indexes.iter().copied())
            .filter(|i| self.socket(*i).is_some() && !self.overrides.contains_key(i))
            .collect();
        for index in indexes {
            let socket = self.socket(index).expect("filtered above");
            let default = Some(socket.entry.single_initial_item_hash)
                .filter(|h| *h != 0)
                .or_else(|| socket.set_plugs.first().copied());
            match default {
                Some(hash) => {
                    self.overrides.insert(index, hash);
                }
                None => self.skip(index as u16, SocketSkipReason::AbilityDefaultUnknown),
            }
        }
    }
    fn categories(&mut self) {
        if self.definition.bucket_hash == Some(SUBCLASS_BUCKET_HASH) {
            self.patch_ability_defaults();
        }
        let categories = self.definition.armor.socket_categories.clone();
        for category in categories {
            let hash = category.socket_category_hash;
            let indexes = category.socket_indexes;
            if ASPECT_CATEGORIES.contains(&hash) {
                self.shuffled(&indexes);
            } else if FRAGMENT_CATEGORIES.contains(&hash) {
                // Only the active fragment sockets count (`loadout-apply.ts:1196-1199`).
                match self.fragment_capacity() {
                    Some(capacity) => {
                        let active = usize::try_from(capacity).unwrap_or(0).min(indexes.len());
                        self.shuffled(&indexes[..active]);
                    }
                    None => {
                        if indexes.iter().any(|i| self.overrides.contains_key(i)) {
                            self.skip(indexes[0] as u16, SocketSkipReason::FragmentCapacityUnknown);
                        }
                    }
                }
            } else {
                // Supers and abilities simply go into their socket.
                for index in indexes {
                    if let (Some(plug), Some(s)) =
                        (self.overrides.get(&index).copied(), self.socket(index))
                    {
                        let socket = s.index;
                        self.keep_or_insert(socket, plug, index as u16);
                    }
                }
            }
        }
    }
    /// DIM's artifact pass (`loadout-apply.ts:1010-1131`): perks may swap sockets, so a
    /// plug already somewhere useful is kept and the override is remapped to it.
    fn artifact(&mut self) {
        let order: Vec<u8> = self.sockets.iter().map(|s| s.index).collect();
        let mut needed: Vec<Needed> = order
            .iter()
            .filter_map(|i| {
                self.overrides.get(&u32::from(*i)).map(|hash| Needed {
                    hash: *hash,
                    loadout: u32::from(*i),
                    original: u16::from(*i),
                })
            })
            .collect();
        for socket in order {
            let Some(current) = self.socket(socket.into()).and_then(|s| s.plugged) else {
                continue;
            };
            let Some(at) = needed.iter().position(|n| n.hash == current) else {
                continue;
            };
            if needed[at].loadout == u32::from(socket) {
                let kept = needed.remove(at);
                self.keep_or_insert(socket, current, kept.original);
                continue;
            }
            let Some(intended) = needed.iter().position(|n| n.loadout == u32::from(socket)) else {
                // Nothing is meant for this socket: the plug stays here.
                let kept = needed.remove(at);
                self.keep_or_insert(socket, current, kept.original);
                continue;
            };
            let accepts = self
                .socket(needed[at].loadout)
                .is_some_and(|other| other.set_plugs.contains(&needed[intended].hash));
            if accepts {
                let kept = needed.remove(at);
                let intended = needed
                    .iter_mut()
                    .find(|n| n.loadout == u32::from(socket))
                    .expect("still present");
                intended.loadout = kept.loadout;
                self.keep_or_insert(socket, current, kept.original);
            } else {
                self.clear(socket);
            }
        }
        for n in needed {
            if let Some(socket) = self.socket(n.loadout).map(|s| s.index) {
                self.keep_or_insert(socket, n.hash, n.original);
            }
        }
    }
}

fn class_matches(
    definition: &ItemDefinition,
    characters: &[Character],
    target: &CharacterId,
) -> bool {
    let class = characters
        .iter()
        .find(|c| &c.id == target)
        .and_then(|c| c.class.as_deref())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(definition.class_type, Some(crate::domain::ClassType::Any))
        || matches!(
            (definition.class_type, class.as_str()),
            (Some(crate::domain::ClassType::Hunter), "hunter")
                | (Some(crate::domain::ClassType::Titan), "titan")
                | (Some(crate::domain::ClassType::Warlock), "warlock")
        )
}
pub(crate) fn sockets<'a>(
    item: &ItemInstance,
    definition: &'a ItemDefinition,
    catalogs: &[ArmorCatalogRecord],
) -> Vec<Socket<'a>> {
    item.sockets
        .iter()
        .filter_map(|s| {
            let entry = definition
                .armor
                .socket_entries
                .get(usize::from(s.socket_index))?;
            let set = entry
                .reusable_plug_set_hash
                .or(entry.randomized_plug_set_hash)
                .and_then(|hash| {
                    catalogs.iter().find_map(|c| match c {
                        ArmorCatalogRecord::PlugSet(p) if p.hash == hash => Some(p),
                        _ => None,
                    })
                });
            let kind = catalogs.iter().find_map(|c| match c {
                ArmorCatalogRecord::SocketType(t) if t.hash == entry.socket_type_hash => Some(t),
                _ => None,
            });
            Some(Socket {
                index: s.socket_index,
                plugged: s.plug_hash.map(|h| h.value() as u32),
                empty: kind.and_then(|t| crate::build_capture::socket_empty_plug(entry, t, set, s)),
                set_plugs: set.map_or_else(Vec::new, |p| {
                    p.reusable_plug_items
                        .iter()
                        .map(|i| i.plug_item_hash)
                        .collect()
                }),
                entry,
            })
        })
        .collect()
}

/// Plans the plug insertions for every selected item that carries socket overrides.
/// The item must be on the target character; DIM only skips vault targets and
/// class-incompatible items, everything else is left to Bungie.
pub fn plan_socket_overrides(
    target: &CharacterId,
    selected: &[ResolvedBuildItem],
    defs: &[ItemDefinition],
    characters: &[Character],
    catalogs: &[ArmorCatalogRecord],
) -> SocketPlan {
    let mut plan = SocketPlan::default();
    for (spec, resolved) in selected.iter().enumerate() {
        if resolved.spec.socket_overrides.is_empty() {
            continue;
        }
        let refused = |reason| SocketSkip {
            spec_index: spec,
            socket_index: None,
            reason,
        };
        let Some(item) = &resolved.inventory else {
            plan.skipped.push(refused(SocketSkipReason::Unresolved));
            continue;
        };
        let Some(definition) = defs.iter().find(|d| d.hash == item.item_hash) else {
            plan.skipped
                .push(refused(SocketSkipReason::MissingDefinition));
            continue;
        };
        let reason = if !class_matches(definition, characters, target) {
            Some(SocketSkipReason::ClassMismatch)
        } else if item.location != ItemLocation::Character(target.clone()) {
            Some(SocketSkipReason::NotOnCharacter)
        } else if item.sockets.is_empty() || definition.armor.socket_categories.is_empty() {
            Some(SocketSkipReason::SocketStateUnavailable)
        } else {
            None
        };
        if let Some(reason) = reason {
            plan.skipped.push(refused(reason));
            continue;
        }
        let mut work = Item {
            spec,
            id: item.instance_id.clone(),
            definition,
            sockets: sockets(item, definition, catalogs),
            overrides: resolved
                .spec
                .socket_overrides
                .iter()
                .map(|(i, h)| (u32::from(*i), h.value() as u32))
                .collect(),
            defs,
            catalogs,
            plan: &mut plan,
        };
        if definition.bucket_hash == Some(ARTIFACT_BUCKET_HASH) {
            work.artifact();
        } else {
            work.categories();
        }
    }
    plan
}
