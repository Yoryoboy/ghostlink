//! DIM-format weapon wish lists: parsing and per-instance roll matching.
//!
//! Format: <https://github.com/DestinyItemManager/DIM/wiki/Creating-Wish-Lists>.
//! Text is kept verbatim; invalid roll lines are skipped whole and counted.

use crate::{AppError, Timestamp};
use std::path::PathBuf;
use std::sync::Arc;

/// DIM's any-item wildcard (`item=-69420`); it is never a trash roll.
pub const WILDCARD_ITEM: i64 = -69420;
/// Lines longer than this are skipped whole, never truncated.
pub const MAX_LINE_CHARS: usize = 16 * 1024;
/// Distinct source/section/notes groups reported per instance.
pub const MAX_REPORTED_MATCHES: usize = 3;
/// `u32::MAX` has 10 digits, so a longer number can never be a hash.
const MAX_NUMBER_DIGITS: usize = 10;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WishlistRoll {
    /// Position among the accepted rolls of one list, in file order.
    pub ordinal: u32,
    /// `None` is the any-item wildcard.
    pub item_hash: Option<u32>,
    pub trash: bool,
    /// Every perk must be offered by the instance; empty matches any roll.
    pub perks: Vec<u32>,
    /// Verbatim notes, including any `|tags:` suffix; rolls under one block
    /// note share it.
    pub notes: Option<Arc<str>>,
    /// The most recent `title:`/`@title:` line before the roll, shared.
    pub section: Option<Arc<str>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WishlistDocument {
    pub title: Option<String>,
    pub description: Option<String>,
    pub rolls: Vec<WishlistRoll>,
    pub skipped: usize,
}

pub fn parse_wishlist(text: &str) -> WishlistDocument {
    let mut document = WishlistDocument::default();
    let mut section: Option<Arc<str>> = None;
    let mut block_notes: Option<Arc<str>> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.chars().count() > MAX_LINE_CHARS {
            if line.starts_with("dimwishlist:") {
                document.skipped += 1;
            }
            block_notes = None;
            continue;
        }
        if let Some(notes) = line.strip_prefix("//notes:") {
            block_notes = Some(Arc::from(notes));
            continue;
        }
        if let Some(roll) = line.strip_prefix("dimwishlist:") {
            match parse_roll(roll) {
                Some((item_hash, trash, perks, line_notes)) => {
                    document.rolls.push(WishlistRoll {
                        ordinal: document.rolls.len() as u32,
                        item_hash,
                        trash,
                        perks,
                        notes: line_notes.map(Arc::from).or_else(|| block_notes.clone()),
                        section: section.clone(),
                    });
                }
                None => document.skipped += 1,
            }
            continue;
        }
        block_notes = None;
        let list_title = line.strip_prefix("title:");
        if let Some(heading) = list_title.or_else(|| line.strip_prefix("@title:")) {
            // Both forms open a section; only a `title:` before any roll also
            // names the whole list.
            if let Some(title) = list_title
                && document.title.is_none()
                && document.rolls.is_empty()
            {
                document.title = Some(title.to_owned());
            }
            section = Some(Arc::from(heading));
        } else if let Some(description) = line.strip_prefix("description:")
            && document.description.is_none()
            && document.rolls.is_empty()
        {
            document.description = Some(description.to_owned());
        }
    }
    document
}

type ParsedRoll = (Option<u32>, bool, Vec<u32>, Option<String>);

fn parse_roll(text: &str) -> Option<ParsedRoll> {
    // DIM accepts a bare `#notes` marker; only `#notes:` carries text.
    let (body, notes) = match text.split_once('#') {
        Some((body, rest)) => (body, rest.strip_prefix("notes:").map(str::to_owned)),
        None => (text, None),
    };
    let body = body.strip_prefix("item=")?;
    let (item, perks) = match body.split_once("&perks=") {
        Some((item, perks)) => (item, Some(perks)),
        None => (body.strip_suffix('&').unwrap_or(body), None),
    };
    let item: i64 = parse_number(item.strip_prefix('-').unwrap_or(item))
        .map(|hash| if item.starts_with('-') { -hash } else { hash })?;
    let perks = match perks {
        Some(perks) => perks
            .split(',')
            .map(|perk| u32::try_from(parse_number(perk)?).ok())
            .collect::<Option<Vec<_>>>()?,
        None => Vec::new(),
    };
    let (item_hash, trash) = if item == WILDCARD_ITEM {
        (None, false)
    } else {
        (Some(u32::try_from(item.unsigned_abs()).ok()?), item < 0)
    };
    // A wildcard without perks would wish-list every item.
    if item_hash.is_none() && perks.is_empty() {
        return None;
    }
    Some((item_hash, trash, perks, notes))
}

fn parse_number(text: &str) -> Option<i64> {
    if text.is_empty()
        || text.len() > MAX_NUMBER_DIGITS
        || !text.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    text.parse().ok()
}

/// Start of the tag list: the last `tags:`, which may lack DIM's `|` prefix.
fn tags_start(notes: &str) -> Option<usize> {
    notes.rfind("tags:")
}

/// Notes without the trailing tag list and its optional `|` separator.
pub fn notes_text(notes: &str) -> &str {
    tags_start(notes).map_or(notes, |at| notes[..at].trim_end_matches('|').trim_end())
}

/// Lower-cased, de-duplicated tags from the last `tags:` list, with DIM
/// community spelling variants folded (`god-pve` → `pve-god`, `m+kb` → `mkb`).
pub fn notes_tags(notes: &str) -> Vec<String> {
    let Some(at) = tags_start(notes) else {
        return Vec::new();
    };
    let mut tags: Vec<String> = Vec::new();
    for token in notes[at + "tags:".len()..]
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ')' | '(' | '|'))
        .filter(|token| !token.is_empty())
    {
        let token = token.to_lowercase();
        let tag = match token.as_str() {
            "god-pve" => "pve-god".to_owned(),
            "god-pvp" => "pvp-god".to_owned(),
            "m+kb" | "mnk" | "mkb" => "mkb".to_owned(),
            _ => token,
        };
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags
}

/// One plug the instance has socketed or can select.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlugOption {
    pub hash: u32,
    pub name: String,
}

impl PlugOption {
    pub fn new(hash: u32, name: impl Into<String>) -> Self {
        Self {
            hash,
            name: name.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WishlistVerdict {
    Wishlist,
    Trash,
    None,
}

impl WishlistVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wishlist => "wishlist",
            Self::Trash => "trash",
            Self::None => "none",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WishlistEvaluation<'a> {
    /// Decided by the first matching roll in list order.
    pub verdict: WishlistVerdict,
    /// The first roll of each distinct section/notes group, bounded.
    pub matches: Vec<&'a WishlistRoll>,
    /// Distinct section/notes groups among all matching rolls.
    pub total: usize,
}

/// Evaluates rolls in list order. A wish-list perk is offered when an option
/// has its hash, or the same name as its definition (enhanced variants).
pub fn evaluate_wishlist<'a>(
    item_hash: u32,
    rolls: impl IntoIterator<Item = &'a WishlistRoll>,
    options: &[PlugOption],
    perk_name: impl Fn(u32) -> Option<String>,
) -> WishlistEvaluation<'a> {
    let (verdict, matches, total) =
        evaluate_keyed(item_hash, rolls, |roll| (roll, None), options, perk_name);
    WishlistEvaluation {
        verdict,
        matches,
        total,
    }
}

/// The shared evaluation over any roll carrier. Matches group by source,
/// section and notes, so two lists citing the same text stay two opinions.
fn evaluate_keyed<'a, T: ?Sized + 'a>(
    item_hash: u32,
    items: impl IntoIterator<Item = &'a T>,
    parts: impl Fn(&'a T) -> (&'a WishlistRoll, Option<&'a str>),
    options: &[PlugOption],
    perk_name: impl Fn(u32) -> Option<String>,
) -> (WishlistVerdict, Vec<&'a T>, usize) {
    let offered = |perk: u32| {
        options.iter().any(|option| option.hash == perk)
            || perk_name(perk).is_some_and(|name| options.iter().any(|option| option.name == name))
    };
    let mut verdict = WishlistVerdict::None;
    let mut groups: Vec<(Option<&str>, Option<&str>, Option<&str>)> = Vec::new();
    let mut matches = Vec::new();
    for item in items {
        let (roll, source) = parts(item);
        if roll.item_hash.is_some_and(|hash| hash != item_hash)
            || !roll.perks.iter().all(|perk| offered(*perk))
        {
            continue;
        }
        if verdict == WishlistVerdict::None {
            verdict = if roll.trash {
                WishlistVerdict::Trash
            } else {
                WishlistVerdict::Wishlist
            };
        }
        let group = (source, roll.section.as_deref(), roll.notes.as_deref());
        if !groups.contains(&group) {
            groups.push(group);
            if matches.len() < MAX_REPORTED_MATCHES {
                matches.push(item);
            }
        }
    }
    (verdict, matches, groups.len())
}

/// The built-in community list: 48klocs/dim-wish-list-sources (MIT), fetched
/// at run time and never embedded.
pub const VOLTRON_URL: &str =
    "https://raw.githubusercontent.com/48klocs/dim-wish-list-sources/master/voltron.txt";
const VOLTRON_KEY: &str = "voltron";
const DISABLED: &str = "none";

/// One configured wish list, applied in configured order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WishlistSource {
    Voltron,
    /// An absolute path to a DIM-format file of the user's own.
    Local(PathBuf),
}

impl WishlistSource {
    /// The stable storage key: `voltron` or the local path.
    pub fn key(&self) -> String {
        match self {
            Self::Voltron => VOLTRON_KEY.to_owned(),
            Self::Local(path) => path.to_string_lossy().into_owned(),
        }
    }

    /// Parses a comma-separated `GHOSTLINK_WISHLISTS` value. Unset or blank
    /// means voltron, `none` alone disables wish lists, and every other entry
    /// must be an absolute local path; remote URLs are never accepted.
    pub fn parse_list(value: Option<&str>) -> Result<Vec<Self>, AppError> {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(vec![Self::Voltron]);
        };
        if value == DISABLED {
            return Ok(Vec::new());
        }
        let mut sources: Vec<Self> = Vec::new();
        for entry in value.split(',').map(str::trim) {
            let source = match entry {
                VOLTRON_KEY => Self::Voltron,
                _ if std::path::Path::new(entry).is_absolute() => Self::Local(PathBuf::from(entry)),
                _ => return Err(AppError::configuration()),
            };
            if sources.contains(&source) {
                return Err(AppError::configuration());
            }
            sources.push(source);
        }
        Ok(sources)
    }
}

/// A change-detection fingerprint (FNV-1a 64 plus byte length). It only
/// avoids re-publishing unchanged content and is not a security digest.
pub fn wishlist_fingerprint(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("fnv1a64:{hash:016x}:{}", bytes.len())
}

/// What one source fetch returned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WishlistFetch {
    /// The stored ETag still matches; the stored rolls stay current.
    NotModified,
    Content {
        text: String,
        etag: Option<String>,
    },
}

pub trait WishlistGateway {
    /// `etag` is the stored validator for a conditional request, if any.
    fn fetch(
        &mut self,
        source: &WishlistSource,
        etag: Option<&str>,
    ) -> Result<WishlistFetch, AppError>;
}

/// Stored metadata of one published source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WishlistSourceRecord {
    pub key: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub etag: Option<String>,
    pub fingerprint: String,
    pub checked_at: Timestamp,
    pub updated_at: Timestamp,
    pub rolls: usize,
    pub skipped: usize,
}

/// A stored roll with the key of the source it came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcedWishlistRoll {
    pub source: String,
    pub roll: WishlistRoll,
}

pub trait WishlistRepository {
    /// Stored sources in configured order.
    fn wishlist_sources(&self) -> Result<Vec<WishlistSourceRecord>, AppError>;
    /// Replaces one source's rolls and metadata atomically.
    fn publish_wishlist(
        &mut self,
        key: &str,
        document: &WishlistDocument,
        etag: Option<&str>,
        fingerprint: &str,
        checked_at: Timestamp,
    ) -> Result<(), AppError>;
    /// Records a check that found unchanged content; rolls are kept.
    fn mark_wishlist_checked(
        &mut self,
        key: &str,
        etag: Option<&str>,
        checked_at: Timestamp,
    ) -> Result<(), AppError>;
    /// Deletes every source not listed and orders the rest as listed.
    fn retain_wishlist_sources(&mut self, keys: &[String]) -> Result<(), AppError>;
    /// The item's rolls plus wildcard rolls, in source order, then file order.
    fn wishlist_rolls(&self, item_hash: u32) -> Result<Vec<SourcedWishlistRoll>, AppError>;
}

/// One distinct matching section/notes group, cited for an agent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WishlistMatch {
    pub source: String,
    pub section: Option<String>,
    /// Definition names of the roll's perks; an unknown hash stays numeric.
    pub perks: Vec<String>,
    /// Notes without the tag list.
    pub notes: Option<String>,
    pub tags: Vec<String>,
}

/// The wish-list opinion on one weapon instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WishlistInspection {
    /// `None` means no list has an opinion, never that the roll is bad.
    pub verdict: WishlistVerdict,
    pub matches: Vec<WishlistMatch>,
    pub matches_total: usize,
}

/// Evaluates loaded rolls (in source, then file order) for a weapon instance.
/// Wish lists cover weapons only, so any other slot yields `None`.
pub fn inspect_wishlist(
    slot: Option<crate::domain::ItemSlot>,
    item_hash: u32,
    rolls: &[SourcedWishlistRoll],
    options: &[PlugOption],
    perk_name: impl Fn(u32) -> Option<String>,
) -> Option<WishlistInspection> {
    if !crate::query::is_weapon_slot(slot) {
        return None;
    }
    let (verdict, matched, matches_total) = evaluate_keyed(
        item_hash,
        rolls,
        |sourced| (&sourced.roll, Some(sourced.source.as_str())),
        options,
        &perk_name,
    );
    let matches = matched
        .into_iter()
        .map(|SourcedWishlistRoll { source, roll }| WishlistMatch {
            source: source.clone(),
            section: roll.section.as_deref().map(str::to_owned),
            perks: roll
                .perks
                .iter()
                .map(|perk| perk_name(*perk).unwrap_or_else(|| perk.to_string()))
                .collect(),
            notes: roll
                .notes
                .as_deref()
                .map(notes_text)
                .filter(|text| !text.is_empty())
                .map(str::to_owned),
            tags: roll.notes.as_deref().map(notes_tags).unwrap_or_default(),
        })
        .collect();
    Some(WishlistInspection {
        verdict,
        matches,
        matches_total,
    })
}
