//! Shared CLI/MCP character selection; no last-played data is available.
use crate::{Character, CharacterId};

/// Selection failures are caller input errors, never provider refusals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WeeklySelectionError {
    InvalidRequest,
    AmbiguousMatch,
}
pub fn select_weekly_character(
    characters: &[Character],
    selector: Option<&CharacterId>,
) -> Result<Character, WeeklySelectionError> {
    let first = characters
        .first()
        .ok_or(WeeklySelectionError::InvalidRequest)?;
    if selector
        .is_some_and(|s| s.as_str().trim().is_empty() || s.as_str().chars().any(char::is_control))
    {
        return Err(WeeklySelectionError::InvalidRequest);
    }
    let Some(selector) = selector else {
        let highest = characters.iter().filter_map(|c| c.light).max();
        let mut winners = characters
            .iter()
            .filter(|c| highest.is_some() && c.light == highest);
        return Ok(match (winners.next(), winners.next()) {
            (Some(winner), None) => winner.clone(),
            _ => first.clone(),
        });
    };
    let exact = characters.iter().any(|c| c.id == *selector);
    let is_class = ["hunter", "titan", "warlock"]
        .iter()
        .any(|class| selector.as_str().eq_ignore_ascii_case(class));
    let mut matches = characters.iter().filter(|c| {
        if exact {
            c.id == *selector
        } else {
            is_class
                && c.class
                    .as_deref()
                    .is_some_and(|class| class.eq_ignore_ascii_case(selector.as_str()))
        }
    });
    let selected = matches.next().ok_or(WeeklySelectionError::InvalidRequest)?;
    if matches.next().is_some() {
        return Err(WeeklySelectionError::AmbiguousMatch);
    }
    Ok(selected.clone())
}
