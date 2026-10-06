//! Community insight contracts, independent of serialization and transport.
use crate::{AppError, Timestamp};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClarityInsight {
    pub hash: u32,
    pub name: String,
    pub kind: String,
    pub item_hash: Option<u32>,
    pub item_name: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClarityMetadata {
    pub version: String,
    pub checked_at: Timestamp,
    pub updated_at: Timestamp,
    pub entries: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClarityBatch {
    pub entries: Vec<ClarityInsight>,
    pub skipped: usize,
}

/// A resolved insight; only an enhanced/base fallback carries a variant marker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClarityMatch {
    pub text: String,
    pub variant: bool,
}

/// Matches verbatim community text without guessing between differing candidates.
pub fn match_clarity(
    hash: u32,
    name: &str,
    item_type: Option<&str>,
    semantic_kind: Option<crate::domain::PlugKind>,
    parent_hash: Option<u32>,
    entries: &[ClarityInsight],
) -> Option<ClarityMatch> {
    if let Some(exact) = entries.iter().find(|entry| entry.hash == hash) {
        return Some(ClarityMatch {
            text: exact.text.clone(),
            variant: false,
        });
    }
    let semantic_kind = semantic_kind.filter(|kind| *kind != crate::domain::PlugKind::Other)?;
    let enhanced = item_type.is_some_and(|kind| kind.starts_with("Enhanced"));
    let named: Vec<_> = entries
        .iter()
        .filter(|entry| entry.name == name && clarity_plug_kind(&entry.kind) == Some(semantic_kind))
        .collect();
    let same_kind: Vec<_> = named
        .iter()
        .copied()
        .filter(|entry| entry.kind.contains("Enhanced") == enhanced)
        .collect();
    let variant = same_kind.is_empty();
    let candidates = if variant { named } else { same_kind };
    let tied: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|entry| parent_hash.is_some() && entry.item_hash == parent_hash)
        .collect();
    let candidates = if tied.is_empty() { candidates } else { tied };
    let first = candidates.first()?;
    candidates
        .iter()
        .all(|entry| entry.text == first.text)
        .then(|| ClarityMatch {
            text: first.text.clone(),
            variant,
        })
}

/// Explicit vaultZero vocabulary mapped to the existing Manifest classification.
/// Unsupported kinds (including catalysts, weapon mods and exotic armor traits)
/// cannot safely participate in name fallback. Exact hashes bypass this mapping.
fn clarity_plug_kind(kind: &str) -> Option<crate::domain::PlugKind> {
    use crate::domain::{PlugKind, SubclassPlugKind};
    match kind {
        "Subclass Aspect" => Some(PlugKind::Subclass(SubclassPlugKind::Aspect)),
        "Subclass Fragment" => Some(PlugKind::Subclass(SubclassPlugKind::Fragment)),
        "Subclass Class" => Some(PlugKind::Subclass(SubclassPlugKind::ClassAbility)),
        "Armor Mod General" => Some(PlugKind::ArmorMod),
        "Weapon Frame"
        | "Weapon Frame Enhanced"
        | "Weapon Frame Exotic"
        | "weaponFrame"
        | "Weapon Perk"
        | "Weapon Perk Exotic"
        | "Weapon Trait"
        | "Weapon Trait Enhanced"
        | "Weapon Trait Exotic"
        | "Weapon Trait Origin"
        | "Weapon Trait Origin Exotic" => Some(PlugKind::WeaponPerk),
        _ => None,
    }
}

pub trait ClarityGateway {
    fn version(&mut self) -> Result<String, AppError>;
    fn descriptions(&mut self) -> Result<ClarityBatch, AppError>;
}

pub trait ClarityRepository {
    fn clarity_by_hash(&self, hash: u32) -> Result<Option<ClarityInsight>, AppError>;
    fn clarity_by_name(&self, name: &str) -> Result<Vec<ClarityInsight>, AppError>;
    fn clarity_metadata(&self) -> Result<Option<ClarityMetadata>, AppError>;
    /// Replaces the entire set and metadata atomically.
    fn publish_clarity(
        &mut self,
        version: &str,
        entries: &[ClarityInsight],
        checked_at: Timestamp,
    ) -> Result<(), AppError>;
    fn mark_clarity_checked(&mut self, checked_at: Timestamp) -> Result<(), AppError>;
}

#[cfg(test)]
mod matching_tests {
    use super::*;
    use crate::domain::{PlugKind, SubclassPlugKind};

    // Most existing matching cases exercise weapon traits.
    fn match_clarity(
        hash: u32,
        name: &str,
        item_type: Option<&str>,
        parent_hash: Option<u32>,
        entries: &[ClarityInsight],
    ) -> Option<ClarityMatch> {
        super::match_clarity(
            hash,
            name,
            item_type,
            Some(PlugKind::WeaponPerk),
            parent_hash,
            entries,
        )
    }

    fn entry(hash: u32, name: &str, kind: &str, text: &str, parent: Option<u32>) -> ClarityInsight {
        ClarityInsight {
            hash,
            name: name.into(),
            kind: kind.into(),
            text: text.into(),
            item_hash: parent,
            item_name: None,
        }
    }

    #[test]
    fn exact_hash_wins_even_over_kind_and_name() {
        let entries = [entry(1, "Other", "Weapon Trait", "  verbatim\n", None)];
        assert_eq!(
            match_clarity(1, "Plug", Some("Enhanced Trait"), None, &entries),
            Some(ClarityMatch {
                text: "  verbatim\n".into(),
                variant: false
            })
        );
    }

    #[test]
    fn prismatic_name_equivalent_and_identical_duplicates() {
        let mut entries = vec![entry(
            4194622039,
            "Ascension",
            "Subclass Aspect",
            "Effect",
            None,
        )];
        entries.push(entry(2, "Ascension", "Subclass Aspect", "Effect", None));
        assert_eq!(
            super::match_clarity(
                2835214901,
                "Ascension",
                Some("Arc Aspect | Light Ability"),
                Some(PlugKind::Subclass(SubclassPlugKind::Aspect)),
                None,
                &entries,
            ),
            Some(ClarityMatch {
                text: "Effect".into(),
                variant: false
            })
        );
    }

    #[test]
    fn enhanced_fallback_is_marked_in_both_directions() {
        let entries = [entry(
            3132659494,
            "Unburied Treasure",
            "Weapon Trait",
            "Base",
            None,
        )];
        assert_eq!(
            match_clarity(
                3804163130,
                "Unburied Treasure",
                Some("Enhanced Trait"),
                None,
                &entries
            ),
            Some(ClarityMatch {
                text: "Base".into(),
                variant: true
            })
        );
        let entries = [entry(
            2,
            "Origin",
            "Weapon Trait Enhanced",
            "Enhanced",
            None,
        )];
        assert!(
            match_clarity(3, "Origin", Some("Trait"), None, &entries)
                .unwrap()
                .variant
        );
        assert!(
            !match_clarity(3, "Origin", Some("Enhanced Origin Trait"), None, &entries)
                .unwrap()
                .variant
        );
    }

    #[test]
    fn differing_texts_never_guess_or_fall_back_to_opposite_kind() {
        let entries = [
            entry(1, "Plug", "Weapon Trait", "A", None),
            entry(2, "Plug", "Weapon Trait", "B", None),
            entry(3, "Plug", "Weapon Trait Enhanced", "C", None),
        ];
        assert_eq!(
            match_clarity(4, "Plug", Some("Trait"), None, &entries),
            None
        );
        assert_eq!(match_clarity(4, "Plug", None, Some(99), &entries), None);
        assert_eq!(
            match_clarity(4, "Plug", Some("Enhanced Trait"), None, &entries[..2]),
            None
        );
    }

    #[test]
    fn parent_preference_and_ambiguous_parent_are_order_independent() {
        let mut entries = vec![
            entry(1, "Plug", "Weapon Trait", "A", None),
            entry(2, "Plug", "Weapon Trait", "B", Some(99)),
        ];
        let expected = Some(ClarityMatch {
            text: "B".into(),
            variant: false,
        });
        assert_eq!(match_clarity(4, "Plug", None, Some(99), &entries), expected);
        entries.reverse();
        assert_eq!(match_clarity(4, "Plug", None, Some(99), &entries), expected);
        entries.push(entry(3, "Plug", "Weapon Trait", "C", Some(99)));
        assert_eq!(match_clarity(4, "Plug", None, Some(99), &entries), None);
    }

    #[test]
    fn same_kind_beats_opposite_kind_even_when_opposite_has_parent() {
        let entries = [
            entry(1, "Plug", "Weapon Trait", "Base", None),
            entry(2, "Plug", "Weapon Trait Enhanced", "Enhanced", Some(99)),
        ];
        assert_eq!(
            match_clarity(3, "Plug", Some("Trait"), Some(99), &entries),
            Some(ClarityMatch {
                text: "Base".into(),
                variant: false
            })
        );
        assert_eq!(
            match_clarity(3, "Plug", Some("Enhanced Trait"), None, &entries),
            Some(ClarityMatch {
                text: "Enhanced".into(),
                variant: false
            })
        );
    }

    #[test]
    fn text_uniqueness_is_verbatim_not_normalized() {
        let entries = [
            entry(1, "Plug", "Weapon Trait", "Text", None),
            entry(2, "Plug", "Weapon Trait", "Text\n", None),
        ];
        assert_eq!(match_clarity(3, "Plug", None, None, &entries), None);
    }

    #[test]
    fn unknown_semantic_kinds_fail_closed_but_exact_hash_still_wins() {
        for kind in [
            "Unknown",
            "Weapon Trait Origin Enhanced",
            "Armor Trait Exotic",
            "Weapon Catalyst Exotic",
            "Weapon Mod",
        ] {
            let entries = [entry(1, "Plug", kind, "Text", None)];
            assert_eq!(match_clarity(2, "Plug", None, None, &entries), None);
            assert_eq!(
                match_clarity(1, "Plug", None, None, &entries).unwrap().text,
                "Text"
            );
        }
        let entries = [entry(1, "Plug", "Weapon Trait", "Text", None)];
        for kind in [None, Some(PlugKind::Other)] {
            assert_eq!(
                super::match_clarity(2, "Plug", None, kind, None, &entries),
                None
            );
            assert!(super::match_clarity(1, "Plug", None, kind, None, &entries).is_some());
        }
    }

    #[test]
    fn unrelated_semantics_are_filtered_before_variant_and_parent_preferences() {
        let entries = [
            entry(1, "Plug", "Weapon Trait Enhanced", "Enhanced", None),
            entry(2, "Plug", "Subclass Aspect", "Unrelated", Some(99)),
        ];
        assert_eq!(
            match_clarity(3, "Plug", Some("Trait"), Some(99), &entries),
            Some(ClarityMatch {
                text: "Enhanced".into(),
                variant: true
            })
        );
        let entries = [
            entry(1, "Plug", "Weapon Trait", "Base", None),
            entry(2, "Plug", "Subclass Aspect", "Unrelated", Some(99)),
        ];
        assert_eq!(
            match_clarity(3, "Plug", Some("Trait"), Some(99), &entries),
            Some(ClarityMatch {
                text: "Base".into(),
                variant: false
            })
        );
    }

    #[test]
    fn observed_vocabulary_maps_only_supported_semantic_kinds() {
        for kind in [
            "Weapon Frame",
            "Weapon Frame Enhanced",
            "Weapon Frame Exotic",
            "weaponFrame",
            "Weapon Perk",
            "Weapon Perk Exotic",
            "Weapon Trait",
            "Weapon Trait Enhanced",
            "Weapon Trait Exotic",
            "Weapon Trait Origin",
            "Weapon Trait Origin Exotic",
        ] {
            assert_eq!(clarity_plug_kind(kind), Some(PlugKind::WeaponPerk));
        }
        for (kind, semantic) in [
            (
                "Subclass Aspect",
                PlugKind::Subclass(SubclassPlugKind::Aspect),
            ),
            (
                "Subclass Fragment",
                PlugKind::Subclass(SubclassPlugKind::Fragment),
            ),
            (
                "Subclass Class",
                PlugKind::Subclass(SubclassPlugKind::ClassAbility),
            ),
            ("Armor Mod General", PlugKind::ArmorMod),
        ] {
            assert_eq!(clarity_plug_kind(kind), Some(semantic));
        }
    }

    #[test]
    fn prismatic_threaded_specter_keeps_aspect_fallback() {
        let entries = [entry(
            4249729125,
            "Threaded Specter",
            "Subclass Aspect",
            "Effect",
            None,
        )];
        assert_eq!(
            super::match_clarity(
                2835214897,
                "Threaded Specter",
                Some("Strand Aspect | Darkness Ability"),
                Some(PlugKind::Subclass(SubclassPlugKind::Aspect)),
                None,
                &entries
            ),
            Some(ClarityMatch {
                text: "Effect".into(),
                variant: false
            })
        );
    }

    #[test]
    fn absent_and_case_different_names_are_not_matches() {
        assert_eq!(match_clarity(1, "Plug", None, None, &[]), None);
        let entries = [entry(2, "plug", "Weapon Trait", "Text", None)];
        assert_eq!(match_clarity(1, "Plug", None, None, &entries), None);
    }
}
