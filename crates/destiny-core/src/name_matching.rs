//! Catalog-based name resolution shared by inventory search surfaces.

use crate::{AppError, ErrorCategory, SearchTerm, normalize_search_text};
use std::collections::BTreeMap;

/// A normalized, distinct catalog built once per kind per query.
#[derive(Default)]
pub struct NameCatalog(BTreeMap<String, String>);
impl NameCatalog {
    pub fn new<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut catalog = BTreeMap::new();
        for name in names {
            if SearchTerm::new(name).is_ok() {
                let display = name.split_whitespace().collect::<Vec<_>>().join(" ");
                catalog
                    .entry(normalize_search_text(name))
                    .or_insert(display);
            }
        }
        Self(catalog)
    }

    pub fn resolve(&self, kind: &str, input: &str) -> Result<Vec<String>, AppError> {
        let input = SearchTerm::new(input)?;
        if let Some(name) = self.0.get(input.as_str()) {
            return Ok(vec![name.clone()]);
        }
        let partial: Vec<_> = self
            .0
            .iter()
            .filter(|(name, _)| name.contains(input.as_str()))
            .take(11)
            .map(|(_, display)| display.clone())
            .collect();
        if partial.len() > 10 {
            return Err(AppError::name_resolution(
                ErrorCategory::AmbiguousSearchName,
                format!(
                    "{} name {:?} matches too many names; be more specific: {}",
                    safe_kind(kind),
                    input.as_str(),
                    partial[..3].join(", ")
                ),
            ));
        }
        if !partial.is_empty() {
            return Ok(partial);
        }
        let mut closest: Vec<_> = self
            .0
            .iter()
            .map(|(name, display)| (bounded_distance(input.as_str(), name), name, display))
            .collect();
        closest.sort();
        let suggestions = closest
            .into_iter()
            .take(3)
            .map(|(_, _, display)| display.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let suffix = if suggestions.is_empty() {
            String::new()
        } else {
            format!("; did you mean: {suggestions}")
        };
        let category = if kind == "stat" {
            ErrorCategory::UnknownStatName
        } else {
            ErrorCategory::UnknownSearchName
        };
        Err(AppError::name_resolution(
            category,
            format!(
                "unknown {} name {:?}{suffix}",
                safe_kind(kind),
                input.as_str()
            ),
        ))
    }
}

pub fn is_name_kind(definition: &crate::ItemDefinition, kind: &str) -> bool {
    match kind {
        "perk" => definition
            .plug_category
            .as_deref()
            .is_some_and(crate::domain::is_weapon_perk_plug),
        "mod" => crate::query::is_named_armor_mod(definition),
        "archetype" => crate::query::is_armor_archetype(definition),
        _ => false,
    }
}

fn safe_kind(kind: &str) -> &str {
    match kind {
        "perk" | "mod" | "archetype" | "stat" => kind,
        _ => "search",
    }
}

// Both operands are bounded by SearchTerm validation (256 Unicode characters).
fn bounded_distance(left: &str, right: &str) -> usize {
    let right: Vec<_> = right.chars().take(crate::MAX_SEARCH_TEXT_LENGTH).collect();
    let mut row: Vec<_> = (0..=right.len()).collect();
    for (i, a) in left.chars().take(crate::MAX_SEARCH_TEXT_LENGTH).enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, b) in right.iter().enumerate() {
            let previous = row[j + 1];
            row[j + 1] = (row[j] + 1)
                .min(previous + 1)
                .min(diagonal + usize::from(a != *b));
            diagonal = previous;
        }
    }
    row[right.len()]
}

/// Display names resolved for each requested filter. Empty fields are omitted by surfaces.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResolvedNames {
    pub perk: Vec<String>,
    pub mods: Vec<String>,
    /// Resolved alternatives for each input term, preserving input order.
    pub perk_terms: Vec<Vec<String>>,
    pub mod_terms: Vec<Vec<String>>,
    pub archetype: Vec<String>,
    pub stats: BTreeMap<String, Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_partial_and_suggestions_are_deterministic() {
        let catalog = NameCatalog::new(["Rampage", "Rampage Spec", "Kill Clip", "RAMPAGE"]);
        assert_eq!(catalog.resolve("perk", " RAMPAGE ").unwrap(), ["Rampage"]);
        assert_eq!(
            catalog.resolve("perk", "ramp").unwrap(),
            ["Rampage", "Rampage Spec"]
        );
        let error = catalog.resolve("perk", "rampge").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown perk name \"rampge\"; did you mean: Rampage")
        );
    }

    #[test]
    fn normalized_names_and_distance_use_unicode() {
        let catalog = NameCatalog::new(["  Élan   Vital ", "Élan Vital", "Class", "Class Ability"]);
        assert_eq!(
            catalog.resolve("perk", "ÉLAN VITAL").unwrap(),
            ["Élan Vital"]
        );
        assert_eq!(bounded_distance("élan", "elan"), 1);
        assert_eq!(bounded_distance("", "élan"), 4);
        let mut term = SearchTerm::new("cla").unwrap();
        assert_eq!(
            term.resolve(&catalog, "stat").unwrap(),
            ["Class", "Class Ability"]
        );
        assert!(term.matches_name("CLASS ABILITY"));
        assert!(!term.matches_name("Class Other"));
        let empty = NameCatalog::new([]);
        assert_eq!(
            empty.resolve("perk", "rampge").unwrap_err().to_string(),
            "unknown perk name \"rampge\""
        );
        let ten: Vec<_> = (0..10).map(|n| format!("Mod {n}")).collect();
        assert_eq!(
            NameCatalog::new(ten.iter().map(String::as_str))
                .resolve("mod", "mod")
                .unwrap()
                .len(),
            10
        );
    }

    #[test]
    fn overbroad_matches_and_controls_are_rejected() {
        let names: Vec<_> = (0..11).map(|n| format!("Mod {n}")).collect();
        let catalog = NameCatalog::new(names.iter().map(String::as_str));
        assert!(
            catalog
                .resolve("mod", "mod")
                .unwrap_err()
                .to_string()
                .contains("be more specific")
        );
        assert_eq!(
            catalog.resolve("mod", "\u{1b}bad").unwrap_err().category(),
            crate::ErrorCategory::ProviderValidation
        );
    }
}
