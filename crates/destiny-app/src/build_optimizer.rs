//! Request-local adapter from projected inventory to DIM's loadout analyzer search
//! (loadout-analyzer/analysis.ts:250-367). Missing data is typed unknown, never guessed.
use destiny_core::{
    armor_catalog::*,
    armor_model::*,
    build::*,
    build_analysis::*,
    build_stats::{font_bonuses, infer_masterwork_assumption, locked_mod_stat_totals},
    mod_assignment::*,
    optimizer::{self, AnalyzerBudget, AnalyzerInputs, ArmorCandidate, LockedExotic, Masterwork},
    *,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const SET_BONUS_SELECTOR_CATEGORY: u32 = 1313063513;
const ARTIFICE_PERK: i64 = 3727270518;
type ArmorPair<'a> = (&'a ItemInstance, &'a ItemDefinition);

/// Why the analyzer could not run: a typed unknown, or a real failure.
pub(crate) enum Stop {
    Unknown(&'static str),
    App(AppError),
}
impl From<AppError> for Stop {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}
impl From<&'static str> for Stop {
    fn from(reason: &'static str) -> Self {
        Self::Unknown(reason)
    }
}

fn settle(
    analysis: &mut BuildAnalysis,
    kind: FindingKind,
    present: Option<bool>,
    reason: &'static str,
) {
    for finding in analysis.findings.iter_mut().filter(|f| f.kind == kind) {
        finding.present = present;
        finding.evaluation = if present.is_some() {
            "evaluated"
        } else {
            "not_evaluated"
        };
        finding.reason = present.is_none().then_some(reason);
    }
}

/// Runs the analyzer for `build check` and settles BetterStatsAvailable and
/// ItemsDoNotMatchSearchQuery. Never touches resolution or apply planning (D3).
#[allow(clippy::too_many_arguments)]
pub(crate) fn evaluate<
    R: DefinitionRepository + StatDefinitionRepository + ArmorCatalogRepository,
>(
    repository: &R,
    build: &BuildDefinition,
    analysis: &mut BuildAnalysis,
    inventory: &[ItemInstance],
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    unlocked: &[u32],
    class: u8,
) -> Result<(), AppError> {
    match run(
        repository,
        build,
        analysis,
        inventory,
        definitions,
        catalogs,
        unlocked,
        class,
    ) {
        Ok(outcome) => {
            let reason = outcome.info.reason.unwrap_or("optimizer_budget_exhausted");
            settle(
                analysis,
                FindingKind::BetterStatsAvailable,
                outcome.better_stats,
                reason,
            );
            settle(
                analysis,
                FindingKind::ItemsDoNotMatchSearchQuery,
                outcome.items_do_not_match,
                reason,
            );
            analysis.optimizer = outcome.info;
        }
        Err(Stop::Unknown(reason)) => {
            settle(analysis, FindingKind::BetterStatsAvailable, None, reason);
            settle(
                analysis,
                FindingKind::ItemsDoNotMatchSearchQuery,
                None,
                reason,
            );
            analysis.optimizer = optimizer::OptimizerAnalysis::not_evaluated(reason);
        }
        Err(Stop::App(error)) => return Err(error),
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run<R: DefinitionRepository + StatDefinitionRepository + ArmorCatalogRepository>(
    repository: &R,
    build: &BuildDefinition,
    analysis: &BuildAnalysis,
    inventory: &[ItemInstance],
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    unlocked: &[u32],
    class: u8,
) -> Result<optimizer::AnalyzerOutcome, Stop> {
    let prepared = prepare(
        repository,
        build,
        analysis,
        inventory,
        definitions,
        catalogs,
        unlocked,
        class,
    )?;
    let matches = prepared.matcher();
    let mut outcome = optimizer::run_analyzer(
        &prepared.inputs(
            prepared
                .has_query
                .then_some(&matches as &dyn Fn(&ArmorCandidate) -> bool),
            AnalyzerBudget::default(),
        ),
    )?;
    outcome.info.font_note = outcome.better_stats == Some(true)
        && font_note(prepared.runtime, &prepared.mod_hashes, &prepared.assumed);
    Ok(outcome)
}

/// Everything the optimizer needs for one build, as DIM's loadout analyzer and Loadout
/// Optimizer both read it from the loadout parameters and the inventory.
pub(crate) struct Prepared {
    pub candidates: Vec<ArmorCandidate>,
    /// Ids of the loadout's five armor pieces.
    pub loadout_ids: Vec<String>,
    matching: BTreeSet<String>,
    pub has_query: bool,
    locked_exotic: Option<LockedExotic>,
    energy_rules: optimizer::EnergyRules,
    saved_set_bonuses: BTreeMap<u32, u32>,
    loadout_set_bonuses: Vec<(u32, u32)>,
    bucket_mods: BTreeMap<i64, Vec<AssignmentMod>>,
    general_mods: Vec<optimizer::ProcessMod>,
    activity_mods: Vec<optimizer::ProcessMod>,
    mod_stat_totals: [i32; 6],
    auto_mod_options: optimizer::AutoModData,
    auto_stat_mods: bool,
    constraints: Vec<optimizer::ResolvedStatConstraint>,
    assumed: [i32; 6],
    runtime: bool,
    mod_hashes: Vec<u32>,
    /// Auto stat mod and tuning definitions fetched beyond the check's own.
    pub extra_definitions: Vec<ItemDefinition>,
}
impl Prepared {
    pub(crate) fn inputs<'a>(
        &'a self,
        query_matches: Option<&'a dyn Fn(&ArmorCandidate) -> bool>,
        budget: AnalyzerBudget,
    ) -> AnalyzerInputs<'a> {
        AnalyzerInputs {
            candidates: &self.candidates,
            loadout_ids: &self.loadout_ids,
            query_matches,
            locked_exotic: self.locked_exotic.clone(),
            energy_rules: self.energy_rules,
            saved_set_bonuses: self.saved_set_bonuses.clone(),
            loadout_set_bonuses: self.loadout_set_bonuses.clone(),
            bucket_mods: self.bucket_mods.clone(),
            general_mods: self.general_mods.clone(),
            activity_mods: self.activity_mods.clone(),
            mod_stat_totals: self.mod_stat_totals,
            auto_mod_options: self.auto_mod_options.clone(),
            auto_stat_mods: self.auto_stat_mods,
            constraints: &self.constraints,
            assumed_stats: self.assumed,
            budget,
            threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
        }
    }
    /// The saved query's verdict for a candidate.
    pub(crate) fn matcher(&self) -> impl Fn(&ArmorCandidate) -> bool + '_ {
        |c| self.matching.contains(&c.id)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare<
    R: DefinitionRepository + StatDefinitionRepository + ArmorCatalogRepository,
>(
    repository: &R,
    build: &BuildDefinition,
    analysis: &BuildAnalysis,
    inventory: &[ItemInstance],
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    unlocked: &[u32],
    class: u8,
) -> Result<Prepared, Stop> {
    // Blocking findings (types.ts:72-76) end the analysis; unknown ones cannot be waved through.
    let blockers = || {
        analysis
            .findings
            .iter()
            .filter(|f| f.kind.blocks_optimization())
    };
    if blockers().any(|f| f.present == Some(true)) {
        return Err("analysis_blocked_by_finding".into());
    }
    if blockers().any(|f| f.present.is_none()) {
        return Err("blocking_finding_not_evaluated".into());
    }
    if let ItemFilter::Unavailable(reason) = analysis.item_filter {
        return Err(reason.into());
    }
    let parameters = build.parameters.as_ref();
    // process.ts perkCountsPrune reads intrinsic perks; Ghostlink has no plug category
    // identifiers, so DIM's `enhancements.exotic` intrinsic fallback cannot be reproduced.
    if parameters
        .and_then(|p| p.perks.as_ref())
        .is_some_and(|p| !p.is_empty())
    {
        return Err("armor_perk_requirements_unsupported".into());
    }
    let assumed = analysis
        .stats
        .assumed
        .ok_or(analysis.stats.reason.unwrap_or("loadout_stats_unavailable"))?;
    let loadout: Vec<ArmorPair> = analysis
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
                .is_some_and(|b| ARMOR_BUCKETS.contains(&b.value()))
        })
        .collect();
    if loadout.len() != 5 {
        return Err("full_resolved_armor_set_required".into());
    }

    // analysis.ts:158-217: the masterwork assumption, including the artifice exotic upgrade.
    let mut assumption = infer_masterwork_assumption(
        parameters
            .and_then(|p| p.assume_armor_masterwork)
            .unwrap_or(AssumeArmorMasterwork::None),
        &loadout,
    );
    let exotic = loadout
        .iter()
        .find(|(_, d)| d.tier.as_deref() == Some("Exotic"));
    if assumption == AssumeArmorMasterwork::All
        && exotic.is_some_and(|(i, _)| is_artifice(i, definitions))
    {
        assumption = AssumeArmorMasterwork::ArtificeExotic;
    }
    let energy_rules = optimizer::EnergyRules {
        assume_armor_masterwork: assumption,
        ..Default::default()
    };

    // matchesExoticArmorHash (analysis.ts:372-396): the exotic that LO must lock.
    let locked_exotic = match parameters.and_then(|p| p.exotic_armor_hash.as_ref()) {
        Some(ExoticArmor::None) => Some(LockedExotic::None),
        Some(ExoticArmor::Any) => Some(LockedExotic::Any),
        Some(ExoticArmor::Item(hash)) => Some(exotic_item(definitions, *hash)?),
        None => exotic.map(|(_, d)| exotic_from(d)),
    };

    // Mods: resolve, drop what DIM drops, split by category (analysis.ts:250-270,323-333).
    let resolved = mod_assignment::resolve_requested_mods(build, definitions, unlocked)?;
    let mut armor_sockets = Vec::new();
    for (item, definition) in &loadout {
        armor_sockets.extend(assignment_sockets(
            item,
            definition,
            definitions,
            catalogs,
            false,
        )?);
    }
    let mods = locked_mods(
        &resolved,
        parameters.and_then(|p| p.auto_stat_mods).unwrap_or(false),
        &armor_sockets,
    );
    let (auto_stat_mods, used) = (mods.auto_stat_mods, mods.used);
    let runtime = parameters
        .and_then(|p| p.include_runtime_stat_benefits)
        .unwrap_or(false);
    let subclass = crate::build_stats::subclass_plugs(analysis, definitions)?;
    let mod_stat_totals = locked_mod_stat_totals(definitions, &subclass, &used, class, runtime)?;

    // Request-local catalogs and definitions beyond the check's own (auto mods, tuning).
    let mut extra = Extra::default();
    let auto_mod_options = if auto_stat_mods {
        let sets: Vec<_> = [
            optimizer::GENERAL_SOCKET_PLUG_SET,
            optimizer::ARTIFICE_SOCKET_PLUG_SET,
        ]
        .into_iter()
        .map(|h| extra.plug_set(repository, catalogs, h))
        .collect::<Result<_, _>>()?;
        let hashes: Vec<_> = sets
            .iter()
            .flatten()
            .flat_map(|s| s.reusable_plug_items.iter().map(|p| p.plug_item_hash))
            .collect();
        extra.definitions(repository, definitions, &hashes)?;
        let records: Vec<_> = sets
            .into_iter()
            .flatten()
            .map(ArmorCatalogRecord::PlugSet)
            .collect();
        let all: Vec<_> = definitions.iter().chain(&extra.defs).cloned().collect();
        optimizer::auto_mod_data(&all, &records, unlocked)?
    } else {
        Default::default()
    };

    // filterItems' input: every armor piece of the class the optimizer can use (analysis.ts:326-331).
    let wanted = [
        domain::ClassType::Titan,
        domain::ClassType::Hunter,
        domain::ClassType::Warlock,
    ]
    .get(usize::from(class))
    .copied();
    let mut candidates = Vec::new();
    let mut pairs: HashMap<String, ArmorPair> = HashMap::new();
    for item in inventory {
        if !matches!(
            item.location,
            ItemLocation::Vault | ItemLocation::Character(_)
        ) {
            continue;
        }
        let Some(definition) = definitions.iter().find(|d| d.hash == item.item_hash) else {
            continue;
        };
        if !definition
            .bucket_hash
            .is_some_and(|b| ARMOR_BUCKETS.contains(&b.value()))
            || definition.class_type != wanted
        {
            continue;
        }
        let mod_sockets = !mods.bucket.is_empty();
        if let Some(candidate) = candidate(
            repository,
            &mut extra,
            definitions,
            catalogs,
            item,
            definition,
            class,
            mod_sockets,
            auto_stat_mods,
        )? {
            pairs.insert(candidate.id.clone(), (item, definition));
            candidates.push(candidate);
        }
    }
    let query = parameters
        .and_then(|p| p.query.as_deref())
        .filter(|q| !q.is_empty());
    let matching: BTreeSet<String> = match &analysis.item_filter {
        ItemFilter::Query(execution) => pairs
            .iter()
            .filter(|(_, (item, definition))| {
                let plugs = |list: &mut dyn Iterator<Item = ItemHash>| -> Vec<ItemDefinition> {
                    list.filter_map(|h| definitions.iter().find(|d| d.hash == h).cloned())
                        .collect()
                };
                let selected = plugs(&mut item.plugs.iter().map(|p| p.plug_hash));
                let available = plugs(&mut item.available_plugs.iter().map(|p| p.plug_hash));
                execution.matches(item, Some(definition), &selected, &available, true, None)
            })
            .map(|(id, _)| id.clone())
            .collect(),
        _ => pairs.keys().cloned().collect(),
    };

    let mut loadout_sets: BTreeMap<u32, u32> = BTreeMap::new();
    for (item, definition) in &loadout {
        let model = crate::armor_model::inspect_armor_model(repository, definition, item)?;
        if let Some(set) = model.and_then(|m| m.armor_set) {
            *loadout_sets.entry(set.hash).or_default() += 1;
        }
    }
    let loadout_ids: Vec<String> = loadout
        .iter()
        .map(|(i, _)| i.instance_id.as_str().to_owned())
        .collect();
    let constraints = optimizer::resolve_stat_constraints(
        parameters
            .and_then(|p| p.stat_constraints.as_deref())
            .unwrap_or(&[]),
    );
    Ok(Prepared {
        candidates,
        loadout_ids,
        matching,
        has_query: query.is_some(),
        locked_exotic,
        energy_rules,
        saved_set_bonuses: parameters
            .and_then(|p| p.set_bonuses.clone())
            .unwrap_or_default(),
        loadout_set_bonuses: loadout_sets.into_iter().collect(),
        bucket_mods: mods.bucket,
        general_mods: mods.general,
        activity_mods: mods.activity,
        mod_stat_totals,
        auto_mod_options,
        auto_stat_mods,
        constraints,
        assumed,
        runtime,
        mod_hashes: resolved.iter().map(|m| m.hash.value() as u32).collect(),
        extra_definitions: extra.defs,
    })
}

const GENERAL_MOD_CATEGORY: u32 = 2487827355;

/// The saved mods as the analyzer hands them to the optimizer.
struct LockedMods<'a> {
    auto_stat_mods: bool,
    /// Mods whose stats count in `modStatChanges`, fitting or not.
    used: Vec<&'a ItemDefinition>,
    general: Vec<optimizer::ProcessMod>,
    activity: Vec<optimizer::ProcessMod>,
    bucket: BTreeMap<i64, Vec<AssignmentMod>>,
}

/// analysis.ts:250-270,323-333: stat mods force auto stat mods on; artifice and tuning
/// mods, and general mods once automatic, are dropped; the rest is categorized against the
/// loadout armor's mod sockets (`categorizeArmorMods`). Mods that fit no socket or have an
/// unknown category are unassigned, which the analyzer ignores.
fn locked_mods<'a>(
    resolved: &[&'a ItemDefinition],
    auto_saved: bool,
    sockets: &[AssignmentSocket],
) -> LockedMods<'a> {
    let category = |m: &ItemDefinition| m.armor.plug_category_hash;
    let auto_stat_mods = auto_saved
        || resolved
            .iter()
            .any(|m| category(m) == Some(GENERAL_MOD_CATEGORY));
    let used: Vec<_> = resolved
        .iter()
        .copied()
        .filter(|m| {
            ![Some(ARTIFICE_CATEGORY), Some(TUNING_CATEGORY)].contains(&category(m))
                && (!auto_stat_mods || category(m) != Some(GENERAL_MOD_CATEGORY))
        })
        .collect();
    let mut mods = LockedMods {
        auto_stat_mods,
        used: used.clone(),
        general: Vec::new(),
        activity: Vec::new(),
        bucket: BTreeMap::new(),
    };
    for m in used.iter().filter(|m| {
        sockets
            .iter()
            .any(|s| s.fits.contains(&(m.hash.value() as u32)))
    }) {
        let Some(category) = category(m) else {
            continue;
        };
        if category == GENERAL_MOD_CATEGORY {
            mods.general.push(optimizer::process_mod(m));
        } else if SPECIALTY_SOCKETS
            .iter()
            .any(|(_, activity)| *activity == category)
        {
            mods.activity.push(optimizer::process_mod(m));
        } else if let Some(bucket) = bucket_for_category(category) {
            let cost = m.armor.energy_cost.as_ref().map_or(0, |e| e.energy_cost);
            mods.bucket.entry(bucket).or_default().push(AssignmentMod {
                hash: m.hash.value() as u32,
                cost,
            });
        }
    }
    mods
}

/// analysis.ts:233-241: a Font mod's runtime bonus sits on a stat that reaches the cap.
fn font_note(runtime: bool, mods: &[u32], assumed: &[i32; 6]) -> bool {
    runtime
        && font_bonuses(mods)
            .iter()
            .zip(assumed)
            .any(|(bonus, stat)| *bonus > 0 && *stat >= optimizer::MAX_STAT)
}

fn exotic_from(definition: &ItemDefinition) -> LockedExotic {
    LockedExotic::Item {
        hash: definition.hash.value() as u32,
        definition: Some(optimizer::ExoticDefinition {
            bucket: definition.bucket_hash.map_or(0, |b| b.value()),
            name: definition.name.clone(),
        }),
    }
}

fn exotic_item(
    definitions: &[ItemDefinition],
    hash: ItemHash,
) -> Result<LockedExotic, &'static str> {
    definitions
        .iter()
        .find(|d| d.hash == hash)
        .map(exotic_from)
        .ok_or("exotic_definition_unavailable")
}

/// item-utils.ts:298-310: a visible artifice plug (or the artifice intrinsic).
fn is_artifice(item: &ItemInstance, definitions: &[ItemDefinition]) -> bool {
    item.sockets
        .iter()
        .filter(|s| s.visible)
        .filter_map(|s| s.plug_hash)
        .any(|hash| {
            hash.value() == ARTIFICE_PERK
                || definitions
                    .iter()
                    .find(|d| d.hash == hash)
                    .is_some_and(|d| d.armor.plug_category_hash == Some(ARTIFICE_CATEGORY))
        })
}

/// Plug sets and plug definitions fetched beyond what the check already loaded.
#[derive(Default)]
struct Extra {
    defs: Vec<ItemDefinition>,
    sets: HashMap<u32, Option<PlugSetDefinition>>,
}
impl Extra {
    fn plug_set<R: ArmorCatalogRepository>(
        &mut self,
        repository: &R,
        catalogs: &[ArmorCatalogRecord],
        hash: u32,
    ) -> Result<Option<PlugSetDefinition>, AppError> {
        if let Some(set) = catalogs.iter().find_map(|c| match c {
            ArmorCatalogRecord::PlugSet(s) if s.hash == hash => Some(s.clone()),
            _ => None,
        }) {
            return Ok(Some(set));
        }
        if let std::collections::hash_map::Entry::Vacant(entry) = self.sets.entry(hash) {
            entry.insert(
                match repository.lookup_armor_catalog(ArmorCatalogKind::PlugSet, hash)? {
                    Some(ArmorCatalogRecord::PlugSet(set)) => Some(set),
                    _ => None,
                },
            );
        }
        Ok(self.sets[&hash].clone())
    }
    fn definitions<R: DefinitionRepository>(
        &mut self,
        repository: &R,
        known: &[ItemDefinition],
        hashes: &[u32],
    ) -> Result<(), AppError> {
        let missing: BTreeSet<ItemHash> = hashes
            .iter()
            .map(|h| ItemHash::new(i64::from(*h)))
            .filter(|h| !known.iter().chain(&self.defs).any(|d| d.hash == *h))
            .collect();
        let missing: Vec<_> = missing.into_iter().collect();
        for chunk in missing.chunks(512) {
            self.defs.extend(repository.lookup_definition_batch(chunk)?);
        }
        Ok(())
    }
}

/// mappers.ts:61-190 inputs for one piece; `None` is not a loadout-builder item
/// (`isLoadoutBuilderItem`: no energy). Missing stat or socket data is a typed unknown.
#[allow(clippy::too_many_arguments)]
fn candidate<R: DefinitionRepository + StatDefinitionRepository + ArmorCatalogRepository>(
    repository: &R,
    extra: &mut Extra,
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    item: &ItemInstance,
    definition: &ItemDefinition,
    class: u8,
    mod_sockets: bool,
    tuning: bool,
) -> Result<Option<ArmorCandidate>, Stop> {
    let Some(energy) = item.energy.as_ref() else {
        return Ok(None);
    };
    let model = crate::armor_model::inspect_armor_model(repository, definition, item)?;
    let Some(model) = model else { return Ok(None) };
    let stats = model.stats.as_ref().ok_or("armor_base_stats_unavailable")?;
    let mut base_stats = [0; 6];
    for (value, hash) in base_stats.iter_mut().zip(ARMOR_STATS) {
        *value = stats
            .stats
            .iter()
            .find(|s| s.stat_hash == hash)
            .ok_or("armor_base_stats_unavailable")?
            .base;
    }
    let stat_plugs = crate::armor_model::resolve_stat_plugs(repository, item, definitions)?;
    let masterwork = match crate::build_stats::masterwork_plug_stats(
        item,
        definitions,
        &stat_plugs,
        stats,
        class,
    )? {
        Some(plug_stats) => Masterwork::Armor3 { plug_stats },
        None => Masterwork::Legacy,
    };
    let mut selector = false;
    for socket in item.sockets.iter().filter(|s| s.visible) {
        let Some(hash) = socket.plug_hash else {
            continue;
        };
        let Some(plug) = definitions.iter().find(|d| d.hash == hash) else {
            // Nameless stat-roll plugs are known and never the set selector.
            if stat_plugs.iter().any(|p| i64::from(p.hash) == hash.value()) {
                continue;
            }
            return Err("inserted_plug_definition_unavailable".into());
        };
        selector |= plug.armor.plug_category_hash == Some(SET_BONUS_SELECTOR_CATEGORY);
    }
    let tuning_plugs = match (&model.tuning, tuning) {
        (Some(socket), true) => Some(tuning_plugs(
            repository,
            extra,
            definitions,
            catalogs,
            item,
            definition,
            socket,
        )?),
        _ => None,
    };
    Ok(Some(ArmorCandidate {
        id: item.instance_id.as_str().to_owned(),
        hash: definition.hash.value() as u32,
        name: definition.name.clone(),
        bucket: definition.bucket_hash.map_or(0, |b| b.value()),
        class_type: class,
        is_exotic: definition.tier.as_deref() == Some("Exotic"),
        is_artifice: is_artifice(item, definitions),
        masterwork,
        tier5: item.gear_tier == Some(5),
        energy_capacity: Some(energy.energy_capacity),
        power: item.power,
        base_stats,
        activity_slot: specialty_category(item, definition, definitions, catalogs)?,
        set_bonus: model.armor_set.as_ref().map(|s| s.hash),
        has_set_bonus_mod_socket: selector,
        tuning: tuning_plugs,
        bucket_sockets: mod_sockets
            .then(|| assignment_sockets(item, definition, definitions, catalogs, true))
            .transpose()?,
        ..Default::default()
    }))
}

/// getArmor3TuningSocket's reusable plugs, restricted to the socket's plug set
/// (mappers.ts:130-160 `plugSet?.plugs`).
fn tuning_plugs<R: DefinitionRepository + ArmorCatalogRepository>(
    repository: &R,
    extra: &mut Extra,
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    item: &ItemInstance,
    definition: &ItemDefinition,
    tuning: &ArmorTuning,
) -> Result<Vec<optimizer::TuningPlug>, Stop> {
    let socket = item
        .sockets
        .iter()
        .find(|s| s.socket_index == tuning.socket_index);
    let reusable = socket
        .and_then(|s| s.reusable_plugs.as_ref())
        .ok_or("tuning_plug_membership_unavailable")?;
    let entry = definition
        .armor
        .socket_entries
        .get(usize::from(tuning.socket_index));
    let set_hash = entry
        .and_then(|e| e.reusable_plug_set_hash.or(e.randomized_plug_set_hash))
        .ok_or("tuning_plug_set_unavailable")?;
    let set = extra
        .plug_set(repository, catalogs, set_hash)?
        .ok_or("tuning_plug_set_unavailable")?;
    let hashes: Vec<u32> = reusable
        .iter()
        .map(|p| p.plug_hash.value() as u32)
        .collect();
    extra.definitions(repository, definitions, &hashes)?;
    Ok(reusable
        .iter()
        .filter(|p| {
            set.reusable_plug_items
                .iter()
                .any(|s| i64::from(s.plug_item_hash) == p.plug_hash.value())
        })
        .map(|p| optimizer::TuningPlug {
            hash: p.plug_hash.value() as u32,
            enabled: p.enabled,
            investment_stats: definitions
                .iter()
                .chain(&extra.defs)
                .find(|d| d.hash == p.plug_hash)
                .map(|d| d.armor.investment_stats.clone()),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{InMemoryDefinitionRepository, synthetic_world};
    use destiny_core::domain::ItemSlot;
    use destiny_core::ports::{
        DefinitionReplacementInput, ManifestDefinitions, WorldDefinitionRepository,
    };

    struct World {
        build: BuildDefinition,
        inventory: Vec<ItemInstance>,
        definitions: Vec<ItemDefinition>,
        catalogs: Vec<ArmorCatalogRecord>,
        repository: InMemoryDefinitionRepository,
    }

    /// A repository holding exactly these definitions and armor catalogs.
    #[rustfmt::skip]
    fn publish(version: &DefinitionVersion, definitions: &[ItemDefinition], catalogs: &[ArmorCatalogRecord]) -> InMemoryDefinitionRepository {
        let mut world = synthetic_world(version.clone());
        world.stats.definitions = ARMOR_STATS.iter().map(|hash| StatDefinition::new(*hash, hash.to_string()).unwrap()).collect();
        world.armor_catalogs = catalogs.to_vec();
        let mut repository = InMemoryDefinitionRepository::default();
        repository.publish_manifest(DefinitionReplacementInput {
            definitions: ManifestDefinitions { version: version.clone(), definitions: definitions.to_vec() },
            checked_at: Timestamp::new(0),
            source: SourceMarker::Manifest,
        }, world).unwrap();
        repository
    }

    /// A Hunter build with five equipped pieces (stats 10/10/10) constrained on three stats,
    /// plus, when set, a locked Vault helmet with that value in the first stat.
    #[rustfmt::skip] // Keep the synthetic armor/catalog fixture compact.
    fn world(extra_helmet: Option<i32>) -> World {
        let version = DefinitionVersion::new("v1");
        let base = ItemDefinition {
            hash: ItemHash::new(1), name: "Armor".into(), bucket_hash: None, tier: Some("Legendary".into()),
            item_type: None, element: None, slot: None, class_type: Some(domain::ClassType::Hunter),
            plug_category: None, tags: vec![], version: version.clone(), armor: Default::default(),
            non_transferrable: None, is_instance_item: Some(true), does_postmaster_pull_have_side_effects: None,
            max_stack_size: None, stack_unique_label: None,
        };
        let mut plug = base.clone();
        plug.hash = ItemHash::new(42);
        plug.armor.plug_category_hash = Some(2912171003);
        let mut definitions = vec![plug];
        let mut inventory = Vec::new();
        let mut build = BuildDefinition {
            class_type: 1,
            parameters: Some(BuildParameters {
                stat_constraints: Some((0..3).map(|i| StatConstraint { stat_hash: ARMOR_STATS[i], min_stat: Some(0), max_stat: None }).collect()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let slots = [ItemSlot::Helmet, ItemSlot::Gauntlets, ItemSlot::Chest, ItemSlot::Legs, ItemSlot::ClassItem];
        let mut pieces: Vec<(usize, i64, i32, bool)> = (0..5).map(|b| (b, b as i64 + 1, 10, true)).collect();
        if let Some(stat) = extra_helmet { pieces.push((0, 11, stat, false)); }
        for (bucket, id, first_stat, equipped) in pieces {
            let mut definition = base.clone();
            definition.hash = ItemHash::new(id);
            definition.bucket_hash = Some(BucketHash::new(mod_assignment::ARMOR_BUCKETS[bucket]));
            definition.slot = Some(slots[bucket]);
            definition.armor.stat_group_hash = Some(100);
            definition.armor.investment_stats = ARMOR_STATS.iter().enumerate().map(|(i, hash)| InvestmentStat {
                stat_type_hash: *hash,
                value: match i { 0 => first_stat, 1 | 2 => 10, _ => 0 },
                is_conditionally_active: false,
            }).collect();
            definition.armor.socket_categories = vec![DefinitionSocketCategory { socket_category_hash: 590099826, socket_indexes: vec![0] }];
            definition.armor.socket_entries = vec![DefinitionSocketEntry { socket_type_hash: 99, plug_sources: Some(2), ..Default::default() }];
            let item = ItemInstance {
                instance_id: ItemInstanceId::new(id.to_string()), item_hash: definition.hash, location: ItemLocation::Vault,
                quantity: 1, equipped: false, locked: !equipped, power: Some(1900), perks: vec![], plugs: vec![], stats: vec![],
                available_plugs: vec![], energy: Some(ItemEnergy { energy_capacity: 10, ..Default::default() }),
                gear_tier: Some(1), transfer_status: Some(0),
                sockets: vec![IndexedSocket {
                    socket_index: 0, plug_hash: Some(ItemHash::new(42)), enabled: true, visible: true, plug_objectives: None,
                    reusable_plugs: Some(vec![domain::ReusablePlug { plug_hash: ItemHash::new(42), enabled: true, can_insert: true }]),
                }],
            };
            if equipped {
                build.equipped.push(BuildItem { instance_id: Some(item.instance_id.clone()), hash: item.item_hash, amount: None, socket_overrides: vec![], crafted_date: None });
            }
            inventory.push(item);
            definitions.push(definition);
        }
        let catalogs = vec![
            ArmorCatalogRecord::SocketType(SocketTypeDefinition { hash: 99, socket_category_hash: 101, always_randomize_sockets: false, plug_whitelist: vec![] }),
            ArmorCatalogRecord::SocketCategory(SocketCategoryDefinition { hash: 101, category_style: 3, name: "Mods".into() }),
            ArmorCatalogRecord::StatGroup(StatGroupDefinition {
                hash: 100, maximum_value: Some(200),
                scaled_stats: ARMOR_STATS.iter().map(|hash| ScaledStat { stat_hash: *hash, maximum_value: 200, display_as_numeric: true, display_interpolation: vec![] }).collect(),
            }),
        ];
        let repository = publish(&version, &definitions, &catalogs);
        World { build, inventory, definitions, catalogs, repository }
    }

    /// The same pipeline as `BuildService::analyze`, up to and including the optimizer.
    fn analyzed(w: &World) -> BuildAnalysis {
        let mut analysis = analyze_build(
            &w.build,
            &CharacterId::new("1"),
            &w.inventory,
            &w.definitions,
            &[],
            &[],
            &[],
        );
        analysis.mod_fit = mod_assignment::analyze_bucket_mods(
            &w.build,
            &analysis.items,
            &w.inventory,
            &w.definitions,
            &w.catalogs,
        );
        crate::build_mods::findings(&w.build, &mut analysis);
        crate::build_stats::evaluate(
            &w.repository,
            &w.build,
            &mut analysis,
            &w.definitions,
            &w.catalogs,
            &[],
            1,
        )
        .unwrap();
        analysis
    }
    fn optimize(w: &World, analysis: &mut BuildAnalysis) {
        evaluate(
            &w.repository,
            &w.build,
            analysis,
            &w.inventory,
            &w.definitions,
            &w.catalogs,
            &[],
            1,
        )
        .unwrap();
    }
    fn finding(
        analysis: &BuildAnalysis,
        kind: FindingKind,
    ) -> (Option<bool>, &'static str, Option<&'static str>) {
        let f = analysis.findings.iter().find(|f| f.kind == kind).unwrap();
        (f.present, f.evaluation, f.reason)
    }

    #[test]
    fn the_equipped_set_alone_has_no_better_stats() {
        let w = world(None);
        let mut analysis = analyzed(&w);
        assert_eq!(analysis.stats.evaluation, "evaluated");
        optimize(&w, &mut analysis);
        assert_eq!(
            finding(&analysis, FindingKind::BetterStatsAvailable),
            (Some(false), "evaluated", None)
        );
        assert_eq!(
            finding(&analysis, FindingKind::ItemsDoNotMatchSearchQuery),
            (Some(false), "evaluated", None)
        );
        assert_eq!(analysis.optimizer.evaluation, "evaluated");
        assert_eq!(analysis.optimizer.candidates, Some([1; 5]));
        assert!(!analysis.optimizer.truncated);
    }

    #[test]
    fn a_better_vault_piece_raises_better_stats_available() {
        let w = world(Some(30));
        let mut analysis = analyzed(&w);
        optimize(&w, &mut analysis);
        assert_eq!(
            finding(&analysis, FindingKind::BetterStatsAvailable),
            (Some(true), "evaluated", None)
        );
        assert_eq!(analysis.optimizer.candidates, Some([1; 5]));
    }

    #[test]
    fn a_blocking_finding_skips_the_analysis_and_states_why() {
        let w = world(Some(30));
        let mut analysis = analyzed(&w);
        analysis
            .findings
            .iter_mut()
            .find(|f| f.kind == FindingKind::ModsDontFit)
            .unwrap()
            .present = Some(true);
        optimize(&w, &mut analysis);
        for kind in [
            FindingKind::BetterStatsAvailable,
            FindingKind::ItemsDoNotMatchSearchQuery,
        ] {
            assert_eq!(
                finding(&analysis, kind),
                (None, "not_evaluated", Some("analysis_blocked_by_finding"))
            );
        }
        assert_eq!(
            analysis.optimizer.reason,
            Some("analysis_blocked_by_finding")
        );
        // An undecided blocker cannot be waved through either.
        analysis
            .findings
            .iter_mut()
            .find(|f| f.kind == FindingKind::ModsDontFit)
            .unwrap()
            .present = None;
        optimize(&w, &mut analysis);
        assert_eq!(
            analysis.optimizer.reason,
            Some("blocking_finding_not_evaluated")
        );
    }

    #[test]
    fn missing_loadout_stats_and_unsupported_inputs_are_typed_unknowns() {
        let mut w = world(Some(30));
        let mut analysis = analyzed(&w);
        analysis.stats = Default::default();
        optimize(&w, &mut analysis);
        assert_eq!(
            analysis.optimizer.reason,
            Some("dim_loadout_stat_aggregation_not_implemented")
        );
        assert_eq!(
            finding(&analysis, FindingKind::BetterStatsAvailable).0,
            None
        );
        // Required perks need DIM's intrinsic-perk lookup, which Ghostlink cannot reproduce.
        w.build.parameters.as_mut().unwrap().perks = Some(vec![PerkHash::new(7)]);
        let mut analysis = analyzed(&w);
        optimize(&w, &mut analysis);
        assert_eq!(
            analysis.optimizer.reason,
            Some("armor_perk_requirements_unsupported")
        );
    }

    #[test]
    fn a_query_that_drops_an_equipped_piece_is_reported_without_better_stats() {
        // Only the worse, locked Vault helmet matches `is:locked`, so the equipped one is out.
        let mut w = world(Some(5));
        w.build.parameters.as_mut().unwrap().query = Some("is:locked".into());
        let mut analysis = analyzed(&w);
        optimize(&w, &mut analysis);
        assert_eq!(
            finding(&analysis, FindingKind::BetterStatsAvailable),
            (Some(false), "evaluated", None)
        );
        assert_eq!(
            finding(&analysis, FindingKind::ItemsDoNotMatchSearchQuery),
            (Some(true), "evaluated", None)
        );
        // An unsupported query capability leaves both findings undecided with its reason.
        w.build.parameters.as_mut().unwrap().query = Some("is:crafted".into());
        let mut analysis = analyzed(&w);
        optimize(&w, &mut analysis);
        let reason = Some("search_engine_capability_unavailable");
        assert_eq!(
            finding(&analysis, FindingKind::BetterStatsAvailable),
            (None, "not_evaluated", reason)
        );
        assert_eq!(
            finding(&analysis, FindingKind::ItemsDoNotMatchSearchQuery),
            (None, "not_evaluated", reason)
        );
        // An invalid query is DIM's pass-all stub: the analysis runs and nothing is excluded.
        w.build.parameters.as_mut().unwrap().query = Some("(".into());
        let mut analysis = analyzed(&w);
        optimize(&w, &mut analysis);
        assert_eq!(
            finding(&analysis, FindingKind::InvalidSearchQuery).0,
            Some(true)
        );
        assert_eq!(
            finding(&analysis, FindingKind::ItemsDoNotMatchSearchQuery),
            (Some(false), "evaluated", None)
        );
    }

    #[test]
    fn a_saved_query_excluding_a_loadout_piece_is_reported_unless_better_stats_exist() {
        let mut w = world(Some(30));
        w.build.parameters.as_mut().unwrap().query = Some("is:armor".into());
        // The query matches every piece here, so the stat-lower pre-filter alone drops the
        // equipped helmet; the better-stats finding then replaces the items finding.
        let mut analysis = analyzed(&w);
        optimize(&w, &mut analysis);
        assert_eq!(
            finding(&analysis, FindingKind::BetterStatsAvailable).0,
            Some(true)
        );
        assert_eq!(
            finding(&analysis, FindingKind::ItemsDoNotMatchSearchQuery),
            (Some(false), "evaluated", None)
        );
    }

    fn mod_definition(hash: i64, category: u32, cost: i32) -> ItemDefinition {
        let mut definition = world(None).definitions[0].clone();
        definition.hash = ItemHash::new(hash);
        definition.armor.plug_category_hash = Some(category);
        definition.armor.energy_cost = Some(PlugEnergyCost {
            energy_cost: cost,
            energy_type: 0,
            energy_type_hash: 0,
        });
        definition
    }

    #[test]
    fn locked_mods_follow_the_analyzers_filtering_and_categories() {
        let helmet = mod_definition(1, 2912171003, 4);
        let general = mod_definition(2, GENERAL_MOD_CATEGORY, 3);
        let artifice = mod_definition(3, ARTIFICE_CATEGORY, 0);
        let tuning = mod_definition(4, TUNING_CATEGORY, 0);
        let maverick = mod_definition(5, 1081029832, 2);
        let unknown = mod_definition(6, 999, 1);
        let misfit = mod_definition(7, 3422420680, 1);
        let socket = AssignmentSocket {
            index: 0,
            plug_set_size: None,
            fits: vec![1, 2, 3, 4, 5, 6],
        };
        let all = [
            &helmet, &general, &artifice, &tuning, &maverick, &unknown, &misfit,
        ];
        let mods = locked_mods(&all, false, std::slice::from_ref(&socket));
        // A general stat mod forces automatic stat mods; it and the free mods are dropped.
        assert!(mods.auto_stat_mods);
        assert!(mods.general.is_empty());
        let hashes = |m: &[&ItemDefinition]| m.iter().map(|d| d.hash.value()).collect::<Vec<_>>();
        assert_eq!(hashes(&mods.used), vec![1, 5, 6, 7]);
        // Maverick is a represented activity tag; unknown categories and misfits are unassigned.
        assert_eq!(
            mods.activity
                .iter()
                .map(|m| (m.hash, m.tag))
                .collect::<Vec<_>>(),
            vec![(5, Some(1081029832))]
        );
        assert_eq!(
            mods.bucket,
            BTreeMap::from([(3448274439, vec![AssignmentMod { hash: 1, cost: 4 }])])
        );
        // Without stat mods the saved flag decides, and nothing is forced.
        let mods = locked_mods(&[&helmet], false, std::slice::from_ref(&socket));
        assert!(!mods.auto_stat_mods);
        assert!(locked_mods(&[&helmet], true, &[socket]).auto_stat_mods);
    }

    #[test]
    fn the_font_note_needs_runtime_effects_on_a_capped_stat() {
        let font_of_focus = 4046357305; // first armor stat
        assert!(font_note(true, &[font_of_focus], &[200, 0, 0, 0, 0, 0]));
        assert!(!font_note(true, &[font_of_focus], &[199, 0, 0, 0, 0, 0]));
        assert!(!font_note(true, &[], &[200; 6]));
        assert!(!font_note(false, &[font_of_focus], &[200; 6]));
    }

    // --- BE-008e: the substitutes path (build_substitutes.rs) over the same fixtures. ---

    use crate::build_substitutes::{SubstitutesReport, evaluate as substitutes_for};

    fn substitutes(w: &World, limit: usize) -> SubstitutesReport {
        substitutes_for(
            &w.repository,
            &w.build,
            &analyzed(w),
            &w.inventory,
            &w.definitions,
            &w.catalogs,
            &[],
            1,
            limit,
            AnalyzerBudget::substitutes(),
        )
        .unwrap()
    }
    fn ids(set: &crate::build_substitutes::SubstituteArmorSet) -> Vec<&str> {
        set.pieces.iter().map(|p| p.instance_id.as_str()).collect()
    }

    #[test]
    fn substitutes_rank_a_better_vault_piece_and_flag_the_build_s_own_pieces() {
        let w = world(Some(30));
        let report = substitutes(&w, 10);
        assert_eq!((report.evaluation, report.reason), ("evaluated", None));
        // The vault helmet is better in every stat, so the equipped one is pre-filtered.
        // Armor with 10 energy counts as masterworked: +2 per piece in every stat.
        assert_eq!(report.sets.len(), 1);
        let set = &report.sets[0];
        assert_eq!(ids(set), ["11", "2", "3", "4", "5"]);
        assert_eq!(
            set.pieces.iter().map(|p| p.in_build).collect::<Vec<_>>(),
            [false, true, true, true, true]
        );
        assert_eq!(set.pieces[0].bucket_hash, mod_assignment::ARMOR_BUCKETS[0]);
        assert_eq!(set.stats[0].stat_hash, ARMOR_STATS[0]);
        assert_eq!(
            set.stats[0].name.as_deref(),
            Some(ARMOR_STATS[0].to_string().as_str())
        );
        assert_eq!((set.stats[0].total, set.stats[0].armor_only), (80, 80));
        assert!(set.stat_mods.is_empty() && set.tuning_mods.is_empty());
        assert_eq!(report.optimizer.candidates, Some([1; 5]));
        assert!(!report.optimizer.truncated);
        assert_eq!(report.optimizer.combos_total, Some(1));
    }

    #[test]
    fn substitutes_of_the_unchanged_inventory_are_the_build_itself() {
        let report = substitutes(&world(None), 10);
        assert_eq!(report.sets.len(), 1);
        assert!(report.sets[0].pieces.iter().all(|p| p.in_build));
        assert_eq!(report.limit, 10);
    }

    #[test]
    fn substitutes_state_why_they_could_not_run() {
        let w = world(Some(30));
        let mut analysis = analyzed(&w);
        analysis
            .findings
            .iter_mut()
            .find(|f| f.kind == FindingKind::ModsDontFit)
            .unwrap()
            .present = Some(true);
        let report = substitutes_for(
            &w.repository,
            &w.build,
            &analysis,
            &w.inventory,
            &w.definitions,
            &w.catalogs,
            &[],
            1,
            10,
            AnalyzerBudget::substitutes(),
        )
        .unwrap();
        assert_eq!(report.evaluation, "not_evaluated");
        assert_eq!(report.reason, Some("analysis_blocked_by_finding"));
        assert!(report.sets.is_empty());
        assert_eq!(report.optimizer.evaluation, "not_evaluated");
    }

    const MAJOR: i64 = 5001;
    const MINOR: i64 = 5002;
    const ARTIFICE_MOD: i64 = 5003;
    const FONT: i64 = 4046357305;
    const TUNE_UP: i64 = 309000506; // tunes ARMOR_STATS[?] up and the dump stat down
    const TUNE_OTHER: i64 = 311164277;

    fn stat_mod(hash: i64, category: u32, cost: i32, stat: usize, value: i32) -> ItemDefinition {
        let mut definition = mod_definition(hash, category, cost);
        definition.armor.investment_stats = vec![InvestmentStat {
            stat_type_hash: ARMOR_STATS[stat],
            value,
            is_conditionally_active: false,
        }];
        definition
    }
    fn plug_set(hash: u32, plugs: &[i64]) -> ArmorCatalogRecord {
        ArmorCatalogRecord::PlugSet(PlugSetDefinition {
            hash,
            reusable_plug_items: plugs
                .iter()
                .map(|p| PlugSetItem {
                    plug_item_hash: *p as u32,
                    currently_can_roll: true,
                })
                .collect(),
        })
    }
    /// The world with the auto stat mod plug sets published in the repository only, so the
    /// adapter has to load them (and their mods) itself, as it does for a real manifest.
    fn auto_mod_world(saved_mods: &[i64], min_first_stat: u16) -> World {
        let mut w = world(None);
        let general = [
            stat_mod(MAJOR, GENERAL_MOD_CATEGORY, 4, 0, 10),
            stat_mod(MINOR, GENERAL_MOD_CATEGORY, 3, 0, 5),
        ];
        let artifice = stat_mod(ARTIFICE_MOD, ARTIFICE_CATEGORY, 0, 0, 3);
        // The check itself only knows the saved mods.
        for hash in saved_mods {
            w.definitions.push(match *hash {
                MAJOR => general[0].clone(),
                _ => stat_mod(*hash, GENERAL_MOD_CATEGORY, 0, 1, 0),
            });
        }
        let mut published = w.definitions.clone();
        published.extend(
            general
                .iter()
                .filter(|g| !saved_mods.contains(&g.hash.value()))
                .cloned(),
        );
        published.push(artifice);
        let mut catalogs = w.catalogs.clone();
        catalogs.push(plug_set(
            optimizer::GENERAL_SOCKET_PLUG_SET,
            &[MAJOR, MINOR],
        ));
        catalogs.push(plug_set(
            optimizer::ARTIFICE_SOCKET_PLUG_SET,
            &[ARTIFICE_MOD],
        ));
        w.repository = publish(&DefinitionVersion::new("v1"), &published, &catalogs);
        let parameters = w.build.parameters.as_mut().unwrap();
        parameters.mods = Some(saved_mods.iter().map(|h| ItemHash::new(*h)).collect());
        parameters.include_runtime_stat_benefits = Some(true);
        parameters.stat_constraints.as_mut().unwrap()[0].min_stat = Some(min_first_stat);
        w
    }

    #[test]
    fn substitutes_load_the_real_auto_mod_plug_sets_and_pick_general_stat_mods() {
        // Saved auto stat mods; the armor alone gives 60 in the first stat.
        let mut w = auto_mod_world(&[], 70);
        w.build.parameters.as_mut().unwrap().auto_stat_mods = Some(true);
        let report = substitutes(&w, 10);
        assert_eq!(
            (report.evaluation, report.reason),
            ("evaluated", None),
            "{report:?}"
        );
        let set = &report.sets[0];
        assert_eq!(set.stats[0].armor_only, 60);
        // The search spends all five general slots on the +10 mod (4 energy each).
        assert_eq!(set.stats[0].total, 110);
        assert_eq!(
            set.stat_mods.iter().map(|m| m.hash).collect::<Vec<_>>(),
            vec![MAJOR as u32; 5]
        );
        assert!(
            set.stat_mods
                .iter()
                .all(|m| m.name.as_deref() == Some("Armor"))
        );
        // A minimum the mods cannot reach leaves no set rather than a wrong one.
        let mut w = auto_mod_world(&[], 150);
        w.build.parameters.as_mut().unwrap().auto_stat_mods = Some(true);
        assert!(substitutes(&w, 10).sets.is_empty());
    }

    #[test]
    fn a_saved_general_stat_mod_and_a_font_turn_auto_stat_mods_on_and_leave_the_pool_free() {
        // The saved +10 general stat mod and the Font are dropped from the locked mods
        // (analysis.ts:250-270): the search still has all five general slots.
        let w = auto_mod_world(&[MAJOR, FONT], 70);
        let report = substitutes(&w, 10);
        assert_eq!(
            (report.evaluation, report.reason),
            ("evaluated", None),
            "{report:?}"
        );
        let set = &report.sets[0];
        assert_eq!(set.stats[0].total, 110);
        assert_eq!(set.stat_mods.len(), 5);
    }

    /// Every armor piece gets a visible tuning socket whose reusable plugs are two tuning
    /// mods from the socket's plug set (mappers.ts:130-160).
    fn tuning_world() -> World {
        let mut w = auto_mod_world(&[], 0);
        w.build.parameters.as_mut().unwrap().auto_stat_mods = Some(true);
        w.build.parameters.as_mut().unwrap().mods = None;
        let version = DefinitionVersion::new("v1");
        let tuning = |hash: i64, up: usize, down: usize| {
            let mut definition = mod_definition(hash, TUNING_CATEGORY, 0);
            definition.armor.investment_stats = vec![
                InvestmentStat {
                    stat_type_hash: ARMOR_STATS[up],
                    value: 5,
                    is_conditionally_active: false,
                },
                InvestmentStat {
                    stat_type_hash: ARMOR_STATS[down],
                    value: -5,
                    is_conditionally_active: false,
                },
            ];
            definition
        };
        let tuners = [tuning(TUNE_UP, 0, 5), tuning(TUNE_OTHER, 1, 5)];
        w.definitions.extend(tuners.iter().cloned());
        for definition in w.definitions.iter_mut().filter(|d| d.bucket_hash.is_some()) {
            definition.armor.socket_entries.push(DefinitionSocketEntry {
                socket_type_hash: 99,
                reusable_plug_set_hash: Some(7000),
                plug_sources: Some(2),
                ..Default::default()
            });
        }
        for item in &mut w.inventory {
            item.sockets.push(IndexedSocket {
                socket_index: 1,
                plug_hash: Some(ItemHash::new(TUNE_UP)),
                enabled: true,
                visible: true,
                plug_objectives: None,
                reusable_plugs: Some(
                    tuners
                        .iter()
                        .map(|t| domain::ReusablePlug {
                            plug_hash: t.hash,
                            enabled: true,
                            can_insert: true,
                        })
                        .collect(),
                ),
            });
        }
        let mut catalogs = w.catalogs.clone();
        catalogs.push(plug_set(7000, &[TUNE_UP, TUNE_OTHER]));
        let mut published = w.definitions.clone();
        published.extend([
            stat_mod(MAJOR, GENERAL_MOD_CATEGORY, 4, 0, 10),
            stat_mod(MINOR, GENERAL_MOD_CATEGORY, 3, 0, 5),
            stat_mod(ARTIFICE_MOD, ARTIFICE_CATEGORY, 0, 0, 3),
        ]);
        catalogs.push(plug_set(
            optimizer::GENERAL_SOCKET_PLUG_SET,
            &[MAJOR, MINOR],
        ));
        catalogs.push(plug_set(
            optimizer::ARTIFICE_SOCKET_PLUG_SET,
            &[ARTIFICE_MOD],
        ));
        w.repository = publish(&version, &published, &catalogs);
        w.catalogs = catalogs;
        w
    }

    #[test]
    fn substitutes_pick_tuning_variants_end_to_end() {
        let w = tuning_world();
        let report = substitutes(&w, 10);
        assert_eq!(
            (report.evaluation, report.reason),
            ("evaluated", None),
            "{report:?}"
        );
        let set = &report.sets[0];
        // Each piece is tuned toward the first priority stat: +5 on five pieces.
        assert_eq!(
            set.tuning_mods.iter().map(|m| m.hash).collect::<Vec<_>>(),
            vec![TUNE_UP as u32; 5]
        );
        assert_eq!(set.stats[0].armor_only, 85);
        // Tuning mods are reported apart from the auto stat mods.
        assert!(set.stat_mods.iter().all(|m| m.hash == MAJOR as u32));
    }
}
