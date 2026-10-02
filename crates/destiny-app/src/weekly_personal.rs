//! Reliable weekly classification over fresh, already-resolved character data.
//! No lifetime completion flag participates in the projection or rules.
use crate::{DefinitionResolution, WeeklyModifier};
use destiny_core::{
    AppError, ChallengeProgress, CharacterId, CharacterWeeklySnapshot, DefinitionVersion,
    DestinyMembership, ErrorCategory, ItemHash, MilestonePhase, PublicMilestonesGateway,
    Resolution, WeeklyProfileGateway, ports::WorldDefinitionRepository,
};

/// Selection and membership resolution belong to the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeeklyCharacter {
    pub membership: DestinyMembership,
    pub character_id: CharacterId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersonalReward {
    pub hash: u32,
    pub definition: DefinitionResolution,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersonalObjective {
    pub hash: u32,
    /// The resolved name is progressDescription (may be blank).
    pub definition: DefinitionResolution,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersonalChallenge {
    pub objective_hash: u32,
    /// The resolved name is the objective's progressDescription (may be blank).
    pub definition: DefinitionResolution,
    pub progress: i64,
    pub completion_value: i64,
    pub complete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhaseProgress {
    pub complete: usize,
    pub total: usize,
}

/// Resolved input to the pure classification functions; also a variant's output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersonalActivity {
    pub hash: u32,
    pub definition: DefinitionResolution,
    pub activity_type: DefinitionResolution,
    pub rewards: Vec<PersonalReward>,
    pub modifiers: Vec<WeeklyModifier>,
    /// Public challenge definitions do not imply personal progress.
    pub challenge_objectives: Vec<PersonalObjective>,
    pub challenges: Vec<PersonalChallenge>,
    pub phases: Option<PhaseProgress>,
    pub focused: bool,
    pub recommended_light: Option<i32>,
    pub has_challenge: bool,
}

#[derive(Clone, Eq, PartialEq)]
pub struct PersonalGroup {
    pub root_name: String,
    pub activity_type_name: String,
    pub activities: Vec<PersonalActivity>,
    /// Resolved visible reward names, deduplicated in provider order.
    pub reward_names: Vec<String>,
}

impl std::fmt::Debug for PersonalGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersonalGroup").finish_non_exhaustive()
    }
}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct WeeklyRotators {
    pub expedition: Option<String>,
    pub override_activity: Option<String>,
}

impl std::fmt::Debug for WeeklyRotators {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WeeklyRotators").finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersonalMilestone {
    pub hash: u32,
    pub definition: DefinitionResolution,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeeklyPersonal {
    pub definition_version: DefinitionVersion,
    /// Type-adjacent groups of roots whose visible reward is exactly Weekly Reward.
    pub newest: Vec<PersonalGroup>,
    pub featured_raids: Vec<PersonalGroup>,
    pub featured_dungeons: Vec<PersonalGroup>,
    pub grandmaster_alert: Option<PersonalActivity>,
    pub weekly_rotators: WeeklyRotators,
    pub portal_focus: Vec<PersonalGroup>,
    /// All resolved inputs retained so missing hashes remain explicit.
    pub character_activities: Vec<PersonalActivity>,
    pub public_activities: Vec<PersonalActivity>,
    pub milestones: Vec<PersonalMilestone>,
}

fn name(definition: &DefinitionResolution) -> Option<&str> {
    match definition {
        DefinitionResolution::Resolved { name, .. } => Some(name),
        DefinitionResolution::Unresolved => None,
    }
}

/// Standard/Master and Epic variants share the name before ':' and ' ('.
pub fn activity_root(display_name: &str) -> &str {
    display_name
        .split(':')
        .next()
        .unwrap_or(display_name)
        .split(" (")
        .next()
        .unwrap_or(display_name)
        .trim()
}

pub fn reward_names(rewards: &[PersonalReward]) -> Vec<String> {
    let mut names = Vec::new();
    for reward in rewards {
        if let Some(value) = name(&reward.definition)
            && !names.iter().any(|existing| existing == value)
        {
            names.push(value.to_owned());
        }
    }
    names
}

fn groups<'a>(activities: impl Iterator<Item = &'a PersonalActivity>) -> Vec<PersonalGroup> {
    let mut result: Vec<PersonalGroup> = Vec::new();
    for activity in activities {
        let (Some(display), Some(kind)) =
            (name(&activity.definition), name(&activity.activity_type))
        else {
            continue;
        };
        let root = activity_root(display);
        let index = result
            .iter()
            .position(|g| g.root_name == root && g.activity_type_name == kind);
        let group = if let Some(index) = index {
            &mut result[index]
        } else {
            result.push(PersonalGroup {
                root_name: root.into(),
                activity_type_name: kind.into(),
                activities: Vec::new(),
                reward_names: Vec::new(),
            });
            result.last_mut().expect("just inserted")
        };
        if !group.activities.iter().any(|a| a.hash == activity.hash) {
            group.activities.push(activity.clone());
        }
        for reward in reward_names(&activity.rewards) {
            if !group.reward_names.contains(&reward) {
                group.reward_names.push(reward);
            }
        }
    }
    result
}

/// Newest content requires the exact resolved visible reward 'Weekly Reward'.
pub fn classify_newest(activities: &[PersonalActivity]) -> Vec<PersonalGroup> {
    let mut result = groups(activities.iter().filter(|a| {
        a.rewards
            .iter()
            .any(|r| name(&r.definition) == Some("Weekly Reward"))
    }));
    // Keep each activity type's roots adjacent, preserving first-seen type order.
    let mut types = Vec::new();
    for group in &result {
        if !types.contains(&group.activity_type_name) {
            types.push(group.activity_type_name.clone());
        }
    }
    result.sort_by_key(|g| types.iter().position(|t| t == &g.activity_type_name));
    result
}

fn featured(
    activities: &[PersonalActivity],
    newest: &[PersonalGroup],
    kind: &str,
) -> Vec<PersonalGroup> {
    groups(activities.iter().filter(|a| {
        a.has_challenge
            && name(&a.activity_type) == Some(kind)
            && name(&a.definition).is_some_and(|n| {
                !newest
                    .iter()
                    .any(|g| g.activity_type_name == kind && g.root_name == activity_root(n))
            })
    }))
}

/// Raids come exclusively from public milestones, never the 204 challenge signal.
pub fn classify_featured_raids(
    public: &[PersonalActivity],
    newest: &[PersonalGroup],
) -> Vec<PersonalGroup> {
    featured(public, newest, "Raid")
}

/// Dungeons come from character activities with challenges, excluding newest roots.
pub fn classify_featured_dungeons(
    character: &[PersonalActivity],
    newest: &[PersonalGroup],
) -> Vec<PersonalGroup> {
    featured(character, newest, "Dungeon")
}

/// Only a unique strictly highest-light challenged Mission can be the GM alert.
pub fn classify_grandmaster(character: &[PersonalActivity]) -> Option<PersonalActivity> {
    let candidates: Vec<_> = character
        .iter()
        .filter(|a| {
            name(&a.activity_type) == Some("Mission")
                && a.has_challenge
                && a.recommended_light.is_some()
        })
        .collect();
    let highest = candidates
        .iter()
        .filter_map(|a| a.recommended_light)
        .max()?;
    let mut winners = candidates
        .into_iter()
        .filter(|a| a.recommended_light == Some(highest));
    let winner = winners.next()?;
    if winners.next().is_some() || name(&winner.definition).is_none() {
        return None;
    }
    Some(winner.clone())
}

/// Expedition/Override retain the location after ':', stripping only Epic suffixes.
/// Reclaim deliberately has no rule: its signal has not been established.
pub fn classify_rotators(character: &[PersonalActivity]) -> WeeklyRotators {
    let single = |prefix: &str| {
        let mut roots = Vec::new();
        for activity in character {
            if let Some(display) = name(&activity.definition)
                && display.starts_with(prefix)
            {
                let root = display
                    .split(" (")
                    .next()
                    .unwrap_or(display)
                    .trim()
                    .to_owned();
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
        }
        if roots.len() == 1 { roots.pop() } else { None }
    };
    WeeklyRotators {
        expedition: single("Expedition: "),
        override_activity: single("Override: "),
    }
}

/// Focused activities grouped by root and exact type, excluding Quickplay roots.
pub fn classify_portal(character: &[PersonalActivity]) -> Vec<PersonalGroup> {
    groups(character.iter().filter(|a| {
        a.focused && name(&a.definition).is_some_and(|n| activity_root(n) != "Quickplay")
    }))
}

/// Phase completion is read solely from character milestone phases.
pub fn phase_progress(phases: &[MilestonePhase]) -> PhaseProgress {
    PhaseProgress {
        complete: phases.iter().filter(|p| p.complete).count(),
        total: phases.len(),
    }
}

/// Preserve character-reported challenge values, deduplicating identical observations.
pub fn challenge_progress(
    activities: &[PersonalChallenge],
    milestones: &[PersonalChallenge],
) -> Vec<PersonalChallenge> {
    let mut result = activities.to_vec();
    for challenge in milestones {
        if !result.contains(challenge) {
            result.push(challenge.clone());
        }
    }
    result
}

fn resolved(name: &str, description: &str) -> DefinitionResolution {
    DefinitionResolution::Resolved {
        name: name.into(),
        description: description.into(),
    }
}

pub struct WeeklyPersonalService<P, G, R> {
    public_gateway: P,
    profile_gateway: G,
    repository: R,
}

impl<P: PublicMilestonesGateway, G: WeeklyProfileGateway, R: WorldDefinitionRepository>
    WeeklyPersonalService<P, G, R>
{
    pub fn new(public_gateway: P, profile_gateway: G, repository: R) -> Self {
        Self {
            public_gateway,
            profile_gateway,
            repository,
        }
    }

    pub fn public_gateway(&self) -> &P {
        &self.public_gateway
    }
    pub fn profile_gateway(&self) -> &G {
        &self.profile_gateway
    }
    pub fn repository(&self) -> &R {
        &self.repository
    }

    /// All five committed world tables and Manifest metadata gate both providers.
    /// Provider/storage errors propagate unchanged; unknown hashes stay unresolved.
    pub fn weekly(&mut self, character: &WeeklyCharacter) -> Result<WeeklyPersonal, AppError> {
        let metadata = self
            .repository
            .current_metadata()?
            .ok_or_else(|| AppError::new(ErrorCategory::DefinitionsUnavailable))?;
        if self.repository.world_definition_counts()?.contains(&0) {
            return Err(AppError::new(ErrorCategory::DefinitionsUnavailable));
        }
        let public = self.public_gateway.load_public_milestones()?;
        public.validate()?;
        let snapshot = self
            .profile_gateway
            .load_character_weekly(&character.membership, &character.character_id)?;
        snapshot.validate()?;
        if snapshot.character_id != character.character_id {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        let mut character_activities = Vec::new();
        for activity in &snapshot.available_activities {
            let mut loaded =
                self.load_activity(activity.activity_hash, &activity.modifier_hashes)?;
            loaded.focused = activity.focused;
            loaded.recommended_light = activity.recommended_light;
            loaded.has_challenge = !activity.challenges.is_empty();
            loaded.challenges = self.load_challenges(&activity.challenges)?;
            for hash in &activity.visible_reward_item_hashes {
                let definition = match self
                    .repository
                    .lookup_definition(ItemHash::new(i64::from(*hash)))?
                {
                    Resolution::Unique(item) => resolved(&item.name, ""),
                    Resolution::Missing => DefinitionResolution::Unresolved,
                    Resolution::Ambiguous(_) => continue,
                };
                loaded.rewards.push(PersonalReward {
                    hash: *hash,
                    definition,
                });
            }
            self.attach_milestone_progress(&mut loaded, &snapshot)?;
            character_activities.push(loaded);
        }
        let mut public_activities = Vec::new();
        let mut milestones = Vec::new();
        for milestone in &public.milestones {
            milestones.push(self.load_milestone(milestone.milestone_hash)?);
            for activity in &milestone.activities {
                let mut loaded =
                    self.load_activity(activity.activity_hash, &activity.modifier_hashes)?;
                loaded.has_challenge = !activity.challenge_objective_hashes.is_empty();
                for hash in &activity.challenge_objective_hashes {
                    loaded.challenge_objectives.push(PersonalObjective {
                        hash: *hash,
                        definition: self.load_objective(*hash)?,
                    });
                }
                if let Some(personal) = character_activities.iter().find(|a| a.hash == loaded.hash)
                {
                    loaded.challenges = personal.challenges.clone();
                    loaded.rewards = personal.rewards.clone();
                    loaded.phases = personal.phases.clone();
                }
                self.attach_milestone_progress(&mut loaded, &snapshot)?;
                public_activities.push(loaded);
            }
        }
        for milestone in &snapshot.milestones {
            if !milestones
                .iter()
                .any(|m| m.hash == milestone.milestone_hash)
            {
                milestones.push(self.load_milestone(milestone.milestone_hash)?);
            }
        }
        let newest = classify_newest(&character_activities);
        Ok(WeeklyPersonal {
            definition_version: metadata.definition_version,
            featured_raids: classify_featured_raids(&public_activities, &newest),
            featured_dungeons: classify_featured_dungeons(&character_activities, &newest),
            grandmaster_alert: classify_grandmaster(&character_activities),
            weekly_rotators: classify_rotators(&character_activities),
            portal_focus: classify_portal(&character_activities),
            newest,
            character_activities,
            public_activities,
            milestones,
        })
    }

    fn load_milestone(&self, hash: u32) -> Result<PersonalMilestone, AppError> {
        Ok(PersonalMilestone {
            hash,
            definition: self
                .repository
                .lookup_milestone(hash)?
                .map_or(DefinitionResolution::Unresolved, |d| {
                    resolved(d.name(), d.description())
                }),
        })
    }

    fn load_activity(&self, hash: u32, modifiers: &[u32]) -> Result<PersonalActivity, AppError> {
        let definition = self.repository.lookup_activity(hash)?;
        let activity_type = match &definition {
            Some(d) => self
                .repository
                .lookup_activity_type(d.activity_type_hash())?
                .map_or(DefinitionResolution::Unresolved, |t| {
                    resolved(t.name(), t.description())
                }),
            None => DefinitionResolution::Unresolved,
        };
        let mut loaded_modifiers = Vec::new();
        for hash in modifiers {
            loaded_modifiers.push(WeeklyModifier {
                hash: *hash,
                definition: self
                    .repository
                    .lookup_activity_modifier(*hash)?
                    .map_or(DefinitionResolution::Unresolved, |d| {
                        resolved(d.name(), d.description())
                    }),
            });
        }
        Ok(PersonalActivity {
            hash,
            definition: definition.map_or(DefinitionResolution::Unresolved, |d| {
                resolved(d.name(), d.description())
            }),
            activity_type,
            modifiers: loaded_modifiers,
            rewards: Vec::new(),
            challenge_objectives: Vec::new(),
            challenges: Vec::new(),
            phases: None,
            focused: false,
            recommended_light: None,
            has_challenge: false,
        })
    }

    fn load_objective(&self, hash: u32) -> Result<DefinitionResolution, AppError> {
        Ok(self
            .repository
            .lookup_objective(hash)?
            .map_or(DefinitionResolution::Unresolved, |d| {
                resolved(d.progress_description(), "")
            }))
    }

    fn load_challenges(
        &self,
        challenges: &[ChallengeProgress],
    ) -> Result<Vec<PersonalChallenge>, AppError> {
        challenges
            .iter()
            .map(|c| {
                Ok(PersonalChallenge {
                    objective_hash: c.objective_hash,
                    definition: self.load_objective(c.objective_hash)?,
                    progress: c.progress,
                    completion_value: c.completion_value,
                    complete: c.complete,
                })
            })
            .collect()
    }

    fn attach_milestone_progress(
        &self,
        loaded: &mut PersonalActivity,
        snapshot: &CharacterWeeklySnapshot,
    ) -> Result<(), AppError> {
        let mut phases = Vec::new();
        let mut present = false;
        for milestone in &snapshot.milestones {
            for activity in &milestone.activities {
                if activity.activity_hash != loaded.hash {
                    continue;
                }
                loaded.challenges = challenge_progress(
                    &loaded.challenges,
                    &self.load_challenges(&activity.challenges)?,
                );
                if name(&loaded.activity_type) == Some("Raid") && !activity.phases.is_empty() {
                    present = true;
                    for phase in &activity.phases {
                        if !phases.contains(phase) {
                            phases.push(phase.clone());
                        }
                    }
                }
            }
        }
        if present {
            loaded.phases = Some(phase_progress(&phases));
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use crate::test_support::{InMemoryDefinitionRepository, synthetic_world};
    use destiny_core::{
        DefinitionReplacementInput, DefinitionRepository, ManifestDefinitions, SourceMarker,
        Timestamp,
    };

    fn activity(hash: u32, name: &str, kind: &str) -> PersonalActivity {
        PersonalActivity {
            hash,
            definition: resolved(name, ""),
            activity_type: resolved(kind, ""),
            rewards: vec![],
            modifiers: vec![],
            challenge_objectives: vec![],
            challenges: vec![],
            phases: None,
            focused: false,
            recommended_light: Some(300),
            has_challenge: true,
        }
    }
    fn reward(name: &str) -> PersonalReward {
        PersonalReward {
            hash: 99,
            definition: resolved(name, ""),
        }
    }
    fn live_week() -> Vec<PersonalActivity> {
        let mut newest_raid = activity(1, "The Desert Perpetual: Standard", "Raid");
        newest_raid.rewards.push(reward("Weekly Reward"));
        let mut newest_dungeon = activity(2, "Equilibrium", "Dungeon");
        newest_dungeon.rewards.push(reward("Weekly Reward"));
        let mut gm = activity(7, "Exodus Crash", "Mission");
        gm.recommended_light = Some(400);
        gm.rewards.push(reward("The Slammer"));
        let mut portal = activity(10, "Portal Mission: Standard", "Mission");
        portal.focused = true;
        portal.rewards.push(reward("The Slammer"));
        let mut quickplay = activity(11, "Quickplay: Vanguard", "Mission");
        quickplay.focused = true;
        vec![
            newest_raid,
            newest_dungeon,
            activity(3, "Deep Stone Crypt: Standard", "Raid"),
            activity(4, "Crota's End: Master", "Raid"),
            activity(5, "Ghosts of the Deep", "Dungeon"),
            activity(6, "Shattered Throne", "Dungeon"),
            gm,
            activity(8, "Expedition: Cosmodrome", "Mission"),
            activity(9, "Override: Europa", "Mission"),
            portal,
            quickplay,
        ]
    }
    #[test]
    fn newest_live_week_uses_exact_reward_and_groups_types() {
        let mut data = live_week();
        data[2].rewards.push(reward("Weekly Reward Gear"));
        let result = classify_newest(&data);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].root_name, "The Desert Perpetual");
        assert_eq!(result[0].activity_type_name, "Raid");
        assert_eq!(result[1].root_name, "Equilibrium");
    }
    #[test]
    fn featured_raids_live_week_public_only_excludes_newest_and_groups_variants() {
        let data = live_week();
        let mut public = vec![
            data[0].clone(),
            data[2].clone(),
            data[3].clone(),
            activity(12, "Crota's End (Epic): Standard", "Raid"),
        ];
        public.push(activity(13, "The Desert Perpetual (Epic): Master", "Raid"));
        public.push(activity(14, "Deep Stone Crypt: Master", "Raid"));
        public.push(data[4].clone());
        public[0].has_challenge = false;
        let result = classify_featured_raids(&public, &classify_newest(&data));
        assert_eq!(
            result
                .iter()
                .map(|g| g.root_name.as_str())
                .collect::<Vec<_>>(),
            ["Deep Stone Crypt", "Crota's End"]
        );
        assert_eq!(result[0].activities.len(), 2);
        assert_eq!(result[1].activities.len(), 2);
    }
    #[test]
    fn featured_dungeons_live_week_excludes_newest() {
        let data = live_week();
        let result = classify_featured_dungeons(&data, &classify_newest(&data));
        assert_eq!(
            result
                .iter()
                .map(|g| g.root_name.as_str())
                .collect::<Vec<_>>(),
            ["Ghosts of the Deep", "Shattered Throne"]
        );
    }
    #[test]
    fn gm_live_week_strict_highest_mission_with_rewards() {
        let data = live_week();
        let result = classify_grandmaster(&data).unwrap();
        assert_eq!(result.hash, 7);
        assert_eq!(reward_names(&result.rewards), ["The Slammer"]);
    }
    #[test]
    fn gm_tie_or_none_never_guesses() {
        let mut data = live_week();
        data[7].recommended_light = Some(400);
        assert!(classify_grandmaster(&data).is_none());
        assert!(classify_grandmaster(&[]).is_none());
    }
    #[test]
    fn rotators_live_week_single_distinct_full_root() {
        let mut data = live_week();
        data.push(activity(15, "Expedition: Cosmodrome (Epic)", "Mission"));
        let result = classify_rotators(&data);
        assert_eq!(result.expedition.as_deref(), Some("Expedition: Cosmodrome"));
        assert_eq!(
            result.override_activity.as_deref(),
            Some("Override: Europa")
        );
    }
    #[test]
    fn rotators_two_expedition_roots_are_omitted() {
        let mut data = live_week();
        data.push(activity(15, "Expedition: Nessus", "Mission"));
        assert!(classify_rotators(&data).expedition.is_none());
    }
    #[test]
    fn portal_live_week_excludes_quickplay_and_unions_rewards_in_order() {
        let mut data = live_week();
        let mut variant = activity(16, "Portal Mission: Master", "Mission");
        variant.focused = true;
        variant.rewards = vec![reward("The Slammer"), reward("Other")];
        data.push(variant);
        let result = classify_portal(&data);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].reward_names, ["The Slammer", "Other"]);
    }
    #[test]
    fn unresolved_hashes_never_classify() {
        let mut data = live_week();
        data[0].rewards[0].definition = DefinitionResolution::Unresolved;
        data[1].activity_type = DefinitionResolution::Unresolved;
        data[6].definition = DefinitionResolution::Unresolved;
        assert!(classify_newest(&data).is_empty());
        assert!(classify_grandmaster(&data).is_none());
    }

    #[test]
    fn featured_raids_exclude_non_newest_public_raid_without_challenge() {
        let mut public = live_week();
        let mut unchallenged = activity(20, "Vault of Glass: Standard", "Raid");
        unchallenged.has_challenge = false;
        public.push(unchallenged);
        let result = classify_featured_raids(&public, &classify_newest(&live_week()));
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|g| g.root_name != "Vault of Glass"));
    }
    #[test]
    fn featured_dungeons_exclude_non_newest_dungeon_without_challenge() {
        let mut character = live_week();
        let mut unchallenged = activity(20, "Pit of Heresy", "Dungeon");
        unchallenged.has_challenge = false;
        character.push(unchallenged);
        let result = classify_featured_dungeons(&character, &classify_newest(&character));
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|g| g.root_name != "Pit of Heresy"));
    }
    #[test]
    fn gm_ignores_higher_light_challenged_non_mission() {
        let mut character = live_week();
        let mut vanguard = activity(20, "Vanguard Op", "Vanguard Op");
        vanguard.recommended_light = Some(450);
        character.push(vanguard);
        assert_eq!(classify_grandmaster(&character).unwrap().hash, 7);
    }
    #[test]
    fn gm_ignores_higher_light_mission_without_challenge() {
        let mut character = live_week();
        let mut mission = activity(20, "Other Mission", "Mission");
        mission.recommended_light = Some(450);
        mission.has_challenge = false;
        character.push(mission);
        assert_eq!(classify_grandmaster(&character).unwrap().hash, 7);
    }
    #[test]
    fn newest_groups_interleaved_roots_by_first_seen_type() {
        let mut character = vec![
            activity(20, "Raid One: Standard", "Raid"),
            activity(21, "Dungeon One", "Dungeon"),
            activity(22, "Raid Two: Standard", "Raid"),
        ];
        for activity in &mut character {
            activity.rewards.push(reward("Weekly Reward"));
        }
        let result = classify_newest(&character);
        let roots_and_types: Vec<_> = result
            .iter()
            .map(|g| (g.root_name.as_str(), g.activity_type_name.as_str()))
            .collect();
        assert_eq!(
            roots_and_types,
            [
                ("Raid One", "Raid"),
                ("Raid Two", "Raid"),
                ("Dungeon One", "Dungeon")
            ]
        );
    }
    #[test]
    fn portal_separates_same_root_with_different_activity_types() {
        let mut character = vec![
            activity(20, "Shared Root: Standard", "Mission"),
            activity(21, "Shared Root: Master", "Vanguard Op"),
        ];
        for activity in &mut character {
            activity.focused = true;
        }
        character[0].rewards.push(reward("Mission Reward"));
        character[1].rewards.push(reward("Op Reward"));
        let result = classify_portal(&character);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].root_name, "Shared Root");
        assert_eq!(result[1].root_name, "Shared Root");
        assert_eq!(result[0].activity_type_name, "Mission");
        assert_eq!(result[1].activity_type_name, "Vanguard Op");
        assert_eq!(result[0].activities[0].hash, 20);
        assert_eq!(result[1].activities[0].hash, 21);
        assert_eq!(result[0].reward_names, ["Mission Reward"]);
        assert_eq!(result[1].reward_names, ["Op Reward"]);
    }

    struct Public {
        result: Result<destiny_core::PublicMilestonesSnapshot, AppError>,
        calls: usize,
    }
    impl PublicMilestonesGateway for Public {
        fn load_public_milestones(
            &mut self,
        ) -> Result<destiny_core::PublicMilestonesSnapshot, AppError> {
            self.calls += 1;
            self.result.clone()
        }
    }
    struct Profile {
        result: Result<CharacterWeeklySnapshot, AppError>,
        calls: usize,
    }
    impl WeeklyProfileGateway for Profile {
        fn load_character_weekly(
            &mut self,
            _: &DestinyMembership,
            _: &CharacterId,
        ) -> Result<CharacterWeeklySnapshot, AppError> {
            self.calls += 1;
            self.result.clone()
        }
    }
    struct Repository {
        inner: InMemoryDefinitionRepository,
        counts: [usize; 5],
        ambiguous_rewards: bool,
    }
    impl DefinitionRepository for Repository {
        fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
            self.inner.current_definition_version()
        }
        fn current_metadata(&self) -> Result<Option<destiny_core::SyncMetadata>, AppError> {
            self.inner.current_metadata()
        }
        fn lookup_definition(
            &self,
            hash: ItemHash,
        ) -> Result<Resolution<destiny_core::ItemDefinition>, AppError> {
            let result = self.inner.lookup_definition(hash)?;
            if self.ambiguous_rewards
                && hash == ItemHash::new(99)
                && let Resolution::Unique(item) = result
            {
                return Ok(Resolution::Ambiguous(vec![item.clone(), item]));
            }
            Ok(result)
        }
        fn replace_definitions(
            &mut self,
            input: DefinitionReplacementInput,
        ) -> Result<destiny_core::SyncMetadata, AppError> {
            self.inner.replace_definitions(input)
        }
    }
    impl WorldDefinitionRepository for Repository {
        fn lookup_milestone(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_core::MilestoneDefinition>, AppError> {
            self.inner.lookup_milestone(hash)
        }
        fn lookup_activity(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_core::ActivityDefinition>, AppError> {
            self.inner.lookup_activity(hash)
        }
        fn lookup_activity_modifier(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_core::ActivityModifierDefinition>, AppError> {
            self.inner.lookup_activity_modifier(hash)
        }
        fn lookup_activity_type(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_core::ActivityTypeDefinition>, AppError> {
            self.inner.lookup_activity_type(hash)
        }
        fn lookup_objective(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_core::ObjectiveDefinition>, AppError> {
            self.inner.lookup_objective(hash)
        }
        fn world_definition_counts(&self) -> Result<[usize; 5], AppError> {
            Ok(self.counts)
        }
        fn publish_manifest(
            &mut self,
            input: DefinitionReplacementInput,
            world: destiny_core::WorldDefinitionsAcquisition,
        ) -> Result<destiny_core::SyncMetadata, AppError> {
            self.inner.publish_manifest(input, world)
        }
    }
    fn character() -> WeeklyCharacter {
        WeeklyCharacter {
            membership: DestinyMembership::new(
                destiny_core::MembershipType::new(3),
                destiny_core::MembershipId::new("123"),
            )
            .unwrap(),
            character_id: CharacterId::new("456"),
        }
    }
    fn service() -> WeeklyPersonalService<Public, Profile, Repository> {
        let version = DefinitionVersion::new("test");
        let mut inner = InMemoryDefinitionRepository::default();
        let mut world = synthetic_world(version.clone());
        world.activities.definitions = live_week()
            .iter()
            .map(|a| {
                let kind = match name(&a.activity_type).unwrap() {
                    "Raid" => 1,
                    "Dungeon" => 2,
                    _ => 3,
                };
                destiny_core::ActivityDefinition::new(
                    a.hash,
                    name(&a.definition).unwrap().into(),
                    String::new(),
                    kind,
                    0,
                )
                .unwrap()
            })
            .collect();
        world.activity_types.definitions = [(1, "Raid"), (2, "Dungeon"), (3, "Mission")]
            .into_iter()
            .map(|(hash, name)| {
                destiny_core::ActivityTypeDefinition::new(hash, name.into(), String::new()).unwrap()
            })
            .collect();
        world.objectives.definitions =
            vec![destiny_core::ObjectiveDefinition::new(50, "Weekly challenge".into(), 1).unwrap()];
        let items = [(99, "Weekly Reward"), (100, "The Slammer")]
            .into_iter()
            .map(|(hash, name)| destiny_core::ItemDefinition {
                hash: ItemHash::new(hash),
                name: name.into(),
                bucket_hash: None,
                item_type: None,
                tier: None,
                element: None,
                tags: vec![],
                version: version.clone(),
            })
            .collect();
        inner
            .publish_manifest(
                DefinitionReplacementInput {
                    definitions: ManifestDefinitions {
                        version: version.clone(),
                        definitions: items,
                    },
                    checked_at: Timestamp::new(0),
                    source: SourceMarker::Manifest,
                },
                world,
            )
            .unwrap();
        let challenge = ChallengeProgress {
            objective_hash: 50,
            progress: 0,
            completion_value: 1,
            complete: false,
        };
        let available_activities = live_week()
            .iter()
            .map(|a| destiny_core::AvailableActivity {
                activity_hash: a.hash,
                focused: a.focused,
                completed_ever: true,
                recommended_light: a.recommended_light,
                difficulty_tier: None,
                modifier_hashes: vec![999],
                challenges: if a.hash == 3 || a.hash == 4 {
                    vec![]
                } else {
                    vec![challenge.clone()]
                },
                visible_reward_item_hashes: a
                    .rewards
                    .iter()
                    .map(|r| {
                        if name(&r.definition) == Some("Weekly Reward") {
                            99
                        } else {
                            100
                        }
                    })
                    .collect(),
            })
            .collect();
        let milestone = destiny_core::CharacterMilestone {
            milestone_hash: 1,
            start: None,
            end: None,
            rewards: vec![],
            activities: vec![destiny_core::CharacterMilestoneActivity {
                activity_hash: 3,
                challenges: vec![challenge],
                phases: vec![
                    MilestonePhase {
                        phase_hash: 20,
                        complete: true,
                    },
                    MilestonePhase {
                        phase_hash: 21,
                        complete: false,
                    },
                ],
            }],
        };
        WeeklyPersonalService::new(
            Public {
                result: Ok(destiny_core::PublicMilestonesSnapshot {
                    milestones: vec![destiny_core::PublicMilestone {
                        milestone_hash: 1,
                        order: 0,
                        start: None,
                        end: None,
                        activities: [1, 3, 4]
                            .into_iter()
                            .map(|hash| destiny_core::PublicMilestoneActivity {
                                activity_hash: hash,
                                modifier_hashes: vec![],
                                challenge_objective_hashes: vec![50],
                            })
                            .collect(),
                    }],
                }),
                calls: 0,
            },
            Profile {
                result: Ok(CharacterWeeklySnapshot {
                    character_id: character().character_id,
                    available_activities,
                    milestones: vec![milestone],
                }),
                calls: 0,
            },
            Repository {
                inner,
                counts: [1; 5],
                ambiguous_rewards: false,
            },
        )
    }
    #[test]
    fn definitions_gate_metadata_and_each_of_five_tables_before_both_providers() {
        for index in 0..6 {
            let mut service = service();
            if index == 5 {
                service.repository.inner = InMemoryDefinitionRepository::default();
            } else {
                service.repository.counts[index] = 0;
            }
            assert_eq!(
                service.weekly(&character()).unwrap_err(),
                AppError::new(ErrorCategory::DefinitionsUnavailable)
            );
            assert_eq!(service.public_gateway.calls, 0);
            assert_eq!(service.profile_gateway.calls, 0);
        }
    }
    #[test]
    fn gateway_errors_propagate_unchanged() {
        let error = AppError::new(ErrorCategory::ProviderUnavailable);
        let mut public_failure = service();
        public_failure.public_gateway.result = Err(error);
        assert_eq!(public_failure.weekly(&character()).unwrap_err(), error);
        assert_eq!(public_failure.profile_gateway.calls, 0);
        let mut profile_failure = service();
        profile_failure.profile_gateway.result = Err(error);
        assert_eq!(profile_failure.weekly(&character()).unwrap_err(), error);
    }
    #[test]
    fn service_live_week_progress_is_character_only_and_not_completed_ever() {
        let result = service().weekly(&character()).unwrap();
        assert_eq!(result.newest.len(), 2);
        assert_eq!(result.featured_raids.len(), 2);
        assert_eq!(result.featured_dungeons.len(), 2);
        assert_eq!(result.grandmaster_alert.as_ref().unwrap().hash, 7);
        let raid = &result.featured_raids[0].activities[0];
        assert_eq!(
            raid.phases,
            Some(PhaseProgress {
                complete: 1,
                total: 2
            })
        );
        assert_eq!(raid.challenges[0].progress, 0);
        assert!(!raid.challenges[0].complete);
        assert_eq!(
            name(&raid.challenges[0].definition),
            Some("Weekly challenge")
        );
        assert_eq!(
            result.newest[0].activities[0].modifiers[0].definition,
            DefinitionResolution::Unresolved
        );
        // Public challenges never fabricate a character's absent progress.
        assert!(result.featured_raids[1].activities[0].challenges.is_empty());
    }
    #[test]
    fn phase_rule_counts_only_reported_completions() {
        assert_eq!(
            phase_progress(&[]),
            PhaseProgress {
                complete: 0,
                total: 0
            }
        );
        assert_eq!(
            phase_progress(&[MilestonePhase {
                phase_hash: 1,
                complete: false
            }]),
            PhaseProgress {
                complete: 0,
                total: 1
            }
        );
    }
    #[test]
    fn challenge_rule_preserves_values_and_deduplicates_observations() {
        let challenge = PersonalChallenge {
            objective_hash: 50,
            definition: resolved("Weekly challenge", ""),
            progress: 2,
            completion_value: 5,
            complete: false,
        };
        let reported = std::slice::from_ref(&challenge);
        assert_eq!(challenge_progress(reported, reported), reported);
        assert_eq!(challenge_progress(&[], reported), reported);
    }
    #[test]
    fn service_reports_unknown_hashes_and_omits_ambiguous_rewards() {
        let mut service = service();
        service.repository.ambiguous_rewards = true;
        let snapshot = service.profile_gateway.result.as_mut().unwrap();
        snapshot.available_activities[0]
            .visible_reward_item_hashes
            .push(999);
        snapshot.available_activities[0].challenges[0].objective_hash = 999;
        snapshot.available_activities[1].activity_hash = 999;
        snapshot.milestones[0].milestone_hash = 999;
        service.public_gateway.result.as_mut().unwrap().milestones[0].activities[0]
            .challenge_objective_hashes = vec![999];
        let result = service.weekly(&character()).unwrap();
        assert!(result.newest.is_empty());
        let activity = &result.character_activities[0];
        assert_eq!(activity.rewards.len(), 1);
        assert_eq!(activity.rewards[0].hash, 999);
        assert_eq!(
            activity.rewards[0].definition,
            DefinitionResolution::Unresolved
        );
        assert_eq!(
            activity.challenges[0].definition,
            DefinitionResolution::Unresolved
        );
        assert_eq!(
            result.character_activities[1].definition,
            DefinitionResolution::Unresolved
        );
        assert_eq!(
            result.character_activities[1].activity_type,
            DefinitionResolution::Unresolved
        );
        assert_eq!(
            result.public_activities[0].challenge_objectives[0].definition,
            DefinitionResolution::Unresolved
        );
        assert_eq!(
            result.milestones.last().unwrap().definition,
            DefinitionResolution::Unresolved
        );
    }
    #[test]
    fn storage_failure_propagates_before_provider_calls() {
        let mut service = service();
        let error = AppError::new(ErrorCategory::LocalStorage);
        service.repository.inner = service.repository.inner.with_metadata_failure(error);
        assert_eq!(service.weekly(&character()).unwrap_err(), error);
        assert_eq!(service.public_gateway.calls, 0);
        assert_eq!(service.profile_gateway.calls, 0);
    }
    #[test]
    fn output_debug_redacts_resolved_text() {
        let result = service().weekly(&character()).unwrap();
        let debug = format!("{result:?}");
        for sentinel in [
            "The Slammer",
            "Weekly Reward",
            "Desert Perpetual",
            "Cosmodrome",
            "Weekly challenge",
        ] {
            assert!(!debug.contains(sentinel));
        }
    }
    #[test]
    fn invalid_public_snapshot_stops_before_profile() {
        let mut service = service();
        service.public_gateway.result.as_mut().unwrap().milestones[0].activities[0]
            .challenge_objective_hashes = vec![50; 65];
        assert_eq!(
            service.weekly(&character()).unwrap_err(),
            AppError::new(ErrorCategory::ProviderValidation)
        );
        assert_eq!(service.profile_gateway.calls, 0);
    }
    #[test]
    fn wrong_character_is_rejected() {
        let mut service = service();
        service
            .profile_gateway
            .result
            .as_mut()
            .unwrap()
            .character_id = CharacterId::new("999");
        assert_eq!(
            service.weekly(&character()).unwrap_err(),
            AppError::new(ErrorCategory::ProviderValidation)
        );
    }
}
