//! DIM-format weapon wish lists: parsing and per-instance roll matching.
//!
//! Format: <https://github.com/DestinyItemManager/DIM/wiki/Creating-Wish-Lists>.
//! Text is kept verbatim; invalid roll lines are skipped whole and counted.

/// DIM's any-item wildcard (`item=-69420`); it is never a trash roll.
pub const WILDCARD_ITEM: i64 = -69420;
/// Lines longer than this are skipped whole, never truncated.
pub const MAX_LINE_CHARS: usize = 16 * 1024;
/// Distinct section/notes groups reported per instance.
pub const MAX_REPORTED_MATCHES: usize = 3;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WishlistRoll {
    /// Position among the accepted rolls of one list, in file order.
    pub ordinal: u32,
    /// `None` is the any-item wildcard.
    pub item_hash: Option<u32>,
    pub trash: bool,
    /// Every perk must be offered by the instance; empty matches any roll.
    pub perks: Vec<u32>,
    /// Verbatim notes, including any `|tags:` suffix.
    pub notes: Option<String>,
    /// The most recent `title:`/`@title:` line before the roll.
    pub section: Option<String>,
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
    let mut section: Option<String> = None;
    let mut block_notes: Option<String> = None;
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
            block_notes = Some(notes.to_owned());
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
                        notes: line_notes.or_else(|| block_notes.clone()),
                        section: section.clone(),
                    });
                }
                None => document.skipped += 1,
            }
            continue;
        }
        block_notes = None;
        if let Some(title) = line
            .strip_prefix("title:")
            .or_else(|| line.strip_prefix("@title:"))
        {
            if document.title.is_none() && line.starts_with("title:") && document.rolls.is_empty() {
                document.title = Some(title.to_owned());
            }
            section = Some(title.to_owned());
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
    if text.is_empty() || text.len() > 10 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Notes without the trailing `|tags:` list.
pub fn notes_text(notes: &str) -> &str {
    notes.rfind("|tags:").map_or(notes, |at| &notes[..at])
}

/// Lower-cased, de-duplicated tags from the last `tags:` list, with DIM
/// community spelling variants folded (`god-pve` → `pve-god`, `m+kb` → `mkb`).
pub fn notes_tags(notes: &str) -> Vec<String> {
    let Some(at) = notes.rfind("tags:") else {
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
    let offered = |perk: u32| {
        options.iter().any(|option| option.hash == perk)
            || perk_name(perk).is_some_and(|name| options.iter().any(|option| option.name == name))
    };
    let mut verdict = WishlistVerdict::None;
    let mut groups: Vec<(&Option<String>, &Option<String>)> = Vec::new();
    let mut matches = Vec::new();
    for roll in rolls {
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
        let group = (&roll.section, &roll.notes);
        if !groups.contains(&group) {
            groups.push(group);
            if matches.len() < MAX_REPORTED_MATCHES {
                matches.push(roll);
            }
        }
    }
    WishlistEvaluation {
        verdict,
        matches,
        total: groups.len(),
    }
}
