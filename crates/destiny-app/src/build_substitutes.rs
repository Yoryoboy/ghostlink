//! `build substitutes`: ranked armor sets for a stored build, as DIM's Loadout Optimizer
//! lists them when opened from that loadout (loadout-builder-reducer.ts:148-232). Read-only:
//! it never touches resolution or the apply plan (D3).
//!
//! Simplification, deliberate: DIM pins the loadout's equipped armor when it opens a loadout
//! (`:185-197`), which would only ever list the loadout itself. Substitutes leave every piece
//! free, as the analyzer does, and flag the pieces that belong to the build.
use crate::build_optimizer::{Stop, prepare};
use destiny_core::{
    armor_catalog::*,
    build::*,
    build_analysis::*,
    optimizer::{self, AnalyzerBudget, ArmorCandidate, OptimizerAnalysis},
    *,
};

/// One armor piece of a substitute set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubstitutePiece {
    pub instance_id: String,
    pub hash: u32,
    pub name: String,
    pub bucket_hash: i64,
    /// The piece is one of the stored build's own armor.
    pub in_build: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubstituteStat {
    pub stat_hash: u32,
    /// The synced stat definition's name, when it is known.
    pub name: Option<String>,
    /// With locked mods, auto stat mods and tuning.
    pub total: i32,
    pub armor_only: i32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubstituteMod {
    pub hash: u32,
    pub name: Option<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubstituteArmorSet {
    pub pieces: Vec<SubstitutePiece>,
    /// The build's priority order.
    pub stats: Vec<SubstituteStat>,
    pub stat_mods: Vec<SubstituteMod>,
    pub tuning_mods: Vec<SubstituteMod>,
    pub power: i32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubstitutesReport {
    /// "evaluated" or "not_evaluated" with a typed reason; never guessed.
    pub evaluation: &'static str,
    pub reason: Option<&'static str>,
    pub limit: usize,
    pub sets: Vec<SubstituteArmorSet>,
    pub optimizer: OptimizerAnalysis,
}

fn not_evaluated(reason: &'static str, limit: usize) -> SubstitutesReport {
    SubstitutesReport {
        evaluation: "not_evaluated",
        reason: Some(reason),
        limit,
        sets: Vec::new(),
        optimizer: OptimizerAnalysis::not_evaluated(reason),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn evaluate<
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
    limit: usize,
    budget: AnalyzerBudget,
) -> Result<SubstitutesReport, AppError> {
    let prepared = match prepare(
        repository,
        build,
        analysis,
        inventory,
        definitions,
        catalogs,
        unlocked,
        class,
    ) {
        Ok(prepared) => prepared,
        Err(Stop::Unknown(reason)) => return Ok(not_evaluated(reason, limit)),
        Err(Stop::App(error)) => return Err(error),
    };
    let matches = prepared.matcher();
    let inputs = prepared.inputs(
        prepared
            .has_query
            .then_some(&matches as &dyn Fn(&ArmorCandidate) -> bool),
        budget,
    );
    let outcome = match optimizer::run_substitutes(&inputs, limit) {
        Ok(outcome) => outcome,
        Err(reason) => return Ok(not_evaluated(reason, limit)),
    };
    let piece = |id: &String| {
        let candidate = prepared.candidates.iter().find(|c| &c.id == id);
        SubstitutePiece {
            instance_id: id.clone(),
            hash: candidate.map_or(0, |c| c.hash),
            name: candidate.map_or_else(String::new, |c| c.name.clone()),
            bucket_hash: candidate.map_or(0, |c| c.bucket),
            in_build: prepared.loadout_ids.contains(id),
        }
    };
    let named = |hash: &u32| SubstituteMod {
        hash: *hash,
        name: definitions
            .iter()
            .chain(&prepared.extra_definitions)
            .find(|d| d.hash.value() == i64::from(*hash))
            .map(|d| d.name.clone()),
    };
    let mut stat_names = Vec::new();
    for hash in outcome.stat_order {
        stat_names.push(repository.lookup_stat(hash)?.map(|d| d.name().to_owned()));
    }
    let sets = outcome
        .sets
        .iter()
        .map(|set| SubstituteArmorSet {
            pieces: set.armor.iter().map(piece).collect(),
            stats: outcome
                .stat_order
                .iter()
                .zip(&stat_names)
                .zip(set.stats.iter().zip(&set.armor_stats))
                .map(|((hash, name), (total, armor))| SubstituteStat {
                    stat_hash: *hash,
                    name: name.clone(),
                    total: *total,
                    armor_only: *armor,
                })
                .collect(),
            stat_mods: set.stat_mods.iter().map(named).collect(),
            tuning_mods: set.tuning_mods.iter().map(named).collect(),
            power: set.power,
        })
        .collect();
    Ok(SubstitutesReport {
        evaluation: "evaluated",
        reason: None,
        limit,
        sets,
        optimizer: outcome.info,
    })
}
