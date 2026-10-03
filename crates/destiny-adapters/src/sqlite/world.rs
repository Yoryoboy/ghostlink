//! World catalogs share the item catalog's transaction and version metadata.
use super::{SqliteStore, storage_failure};
use destiny_core::ports::WorldDefinitionRepository;
use destiny_core::*;
use rusqlite::{Connection, OptionalExtension, params};

pub(super) fn read_stats(
    connection: &Connection,
) -> Result<Vec<(StatDefinition, String)>, AppError> {
    let mut statement = connection
        .prepare("SELECT hash, name, definition_version FROM stat_definitions ORDER BY hash")
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|_| storage_failure())?;
    let metadata = super::definitions::read_manifest_metadata(connection)?;
    let mut stats = Vec::new();
    for row in rows {
        let (hash, name, version) = row.map_err(|_| storage_failure())?;
        if !metadata
            .as_ref()
            .is_some_and(|metadata| metadata.definition_version.as_str() == version)
            || version.is_empty()
            || version.chars().count() > 128
            || version.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(storage_failure());
        }
        stats.push((
            StatDefinition::new(hash, name).map_err(|_| storage_failure())?,
            version,
        ));
        if stats.len() > 1000 {
            return Err(storage_failure());
        }
    }
    Ok(stats)
}

impl destiny_core::ports::StatDefinitionRepository for SqliteStore {
    fn lookup_stat(&self, hash: u32) -> Result<Option<StatDefinition>, AppError> {
        Ok(read_stats(self.connection())?
            .into_iter()
            .find(|(stat, _)| stat.hash() == hash)
            .map(|(stat, _)| stat))
    }
    fn stat_hashes_by_name(&self, name: &str) -> Result<Vec<u32>, AppError> {
        let normalized = normalize_stat_name(name)?;
        let mut hashes = Vec::new();
        for (stat, _) in read_stats(self.connection())? {
            if normalize_stat_name(stat.name()).map_err(|_| storage_failure())? == normalized {
                hashes.push(stat.hash());
            }
        }
        Ok(hashes)
    }
    fn stat_definition_count(&self) -> Result<usize, AppError> {
        Ok(read_stats(self.connection())?.len())
    }
}

macro_rules! lookup {
    ($fn:ident, $kind:ident, $sql:literal $(, $index:literal)*) => {
        fn $fn(&self, hash: u32) -> Result<Option<$kind>, AppError> {
            self.connection().query_row($sql, [hash], |row| {
                let name: String = row.get(0)?;
                let description: String = row.get(1)?;
                let value = $kind::new(hash, name, description $(, row.get($index)?)*);
                Ok(value)
            }).optional().map_err(|_| storage_failure())?
                .transpose().map_err(|_| storage_failure())
        }
    };
}

impl WorldDefinitionRepository for SqliteStore {
    lookup!(
        lookup_milestone,
        MilestoneDefinition,
        "SELECT name, description, milestone_type FROM milestone_definitions WHERE hash = ?1",
        2
    );
    lookup!(
        lookup_activity,
        ActivityDefinition,
        "SELECT name, description, activity_type_hash, light_level FROM activity_definitions WHERE hash = ?1",
        2,
        3
    );
    lookup!(
        lookup_activity_modifier,
        ActivityModifierDefinition,
        "SELECT name, description FROM activity_modifier_definitions WHERE hash = ?1"
    );

    lookup!(
        lookup_activity_type,
        ActivityTypeDefinition,
        "SELECT name, description FROM activity_type_definitions WHERE hash = ?1"
    );
    fn lookup_objective(&self, hash: u32) -> Result<Option<ObjectiveDefinition>, AppError> {
        self.connection().query_row("SELECT progress_description, completion_value FROM objective_definitions WHERE hash = ?1", [hash], |row| {
            Ok(ObjectiveDefinition::new(hash, row.get(0)?, row.get(1)?))
        }).optional().map_err(|_| storage_failure())?.transpose().map_err(|_| storage_failure())
    }
    fn world_definition_counts(&self) -> Result<[usize; 5], AppError> {
        let mut counts = [0; 5];
        for (index, sql) in [
            "SELECT COUNT(*) FROM milestone_definitions",
            "SELECT COUNT(*) FROM activity_definitions",
            "SELECT COUNT(*) FROM activity_modifier_definitions",
            "SELECT COUNT(*) FROM objective_definitions",
            "SELECT COUNT(*) FROM activity_type_definitions",
        ]
        .iter()
        .enumerate()
        {
            let count: i64 = self
                .connection()
                .query_row(sql, [], |row| row.get(0))
                .map_err(|_| storage_failure())?;
            counts[index] = usize::try_from(count).map_err(|_| storage_failure())?;
        }
        Ok(counts)
    }

    fn publish_manifest(
        &mut self,
        items: DefinitionReplacementInput,
        world: WorldDefinitionsAcquisition,
    ) -> Result<SyncMetadata, AppError> {
        self.publish_catalog(items, Some(world))
    }
}

pub(super) fn clear_catalog(connection: &Connection) -> Result<(), AppError> {
    if super::migrations::read_user_version(connection)? >= 8 {
        connection
            .execute("DELETE FROM stat_definitions", [])
            .map_err(|_| storage_failure())?;
    }
    connection.execute_batch("DELETE FROM milestone_definitions; DELETE FROM activity_definitions; DELETE FROM activity_modifier_definitions; DELETE FROM objective_definitions; DELETE FROM activity_type_definitions;")
        .map_err(|_| storage_failure())
}

pub(super) fn replace_catalog(
    connection: &Connection,
    world: &WorldDefinitionsAcquisition,
) -> Result<(), AppError> {
    clear_catalog(connection)?;
    let version = world.version.as_str();
    if super::migrations::read_user_version(connection)? >= 8 {
        for definition in &world.stats.definitions {
            connection
                .execute(
                    "INSERT INTO stat_definitions VALUES (?1, ?2, ?3)",
                    params![definition.hash(), definition.name(), version],
                )
                .map_err(|_| storage_failure())?;
        }
    }
    for definition in &world.milestones.definitions {
        connection
            .execute(
                "INSERT INTO milestone_definitions VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    definition.hash(),
                    definition.name(),
                    definition.description(),
                    definition.milestone_type(),
                    version
                ],
            )
            .map_err(|_| storage_failure())?;
    }
    for definition in &world.activities.definitions {
        connection
            .execute(
                "INSERT INTO activity_definitions VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    definition.hash(),
                    definition.name(),
                    definition.description(),
                    definition.activity_type_hash(),
                    definition.light_level(),
                    version
                ],
            )
            .map_err(|_| storage_failure())?;
    }
    for definition in &world.modifiers.definitions {
        connection
            .execute(
                "INSERT INTO activity_modifier_definitions VALUES (?1, ?2, ?3, ?4)",
                params![
                    definition.hash(),
                    definition.name(),
                    definition.description(),
                    version
                ],
            )
            .map_err(|_| storage_failure())?;
    }
    for definition in &world.objectives.definitions {
        connection
            .execute(
                "INSERT INTO objective_definitions VALUES (?1, ?2, ?3, ?4)",
                params![
                    definition.hash(),
                    definition.progress_description(),
                    definition.completion_value(),
                    version
                ],
            )
            .map_err(|_| storage_failure())?;
    }
    for definition in &world.activity_types.definitions {
        connection
            .execute(
                "INSERT INTO activity_type_definitions VALUES (?1, ?2, ?3, ?4)",
                params![
                    definition.hash(),
                    definition.name(),
                    definition.description(),
                    version
                ],
            )
            .map_err(|_| storage_failure())?;
    }
    Ok(())
}

macro_rules! table_matches {
    ($connection:expr, $world:expr, $field:ident, $sql:literal, $kind:ident $(, $index:literal)*) => {{
        let mut statement = $connection.prepare($sql).map_err(|_| storage_failure())?;
        let rows = statement.query_map([], |row| {
            let value = $kind::new(row.get(0)?, row.get(1)?, row.get(2)? $(, row.get($index)?)*);
            let version: String = row.get(row.as_ref().column_count() - 1)?;
            Ok((value, version))
        }).map_err(|_| storage_failure())?;
        let mut stored = Vec::new();
        for row in rows {
            let (value, version) = row.map_err(|_| storage_failure())?;
            if version != $world.version.as_str() { return Ok(false); }
            stored.push(value.map_err(|_| storage_failure())?);
        }
        let mut incoming = $world.$field.definitions.clone();
        incoming.sort_by_key($kind::hash);
        stored == incoming
    }};
}

pub(super) fn contents_match(
    connection: &Connection,
    world: &WorldDefinitionsAcquisition,
) -> Result<bool, AppError> {
    let stats_match = if super::migrations::read_user_version(connection)? >= 8 {
        let mut incoming = world.stats.definitions.clone();
        incoming.sort_by_key(StatDefinition::hash);
        let stored = read_stats(connection)?;
        stored
            .iter()
            .all(|(_, version)| version == world.version.as_str())
            && stored.into_iter().map(|(stat, _)| stat).collect::<Vec<_>>() == incoming
    } else {
        true
    };
    Ok(stats_match
        && table_matches!(
            connection,
            world,
            milestones,
            "SELECT hash, name, description, milestone_type, definition_version FROM milestone_definitions ORDER BY hash",
            MilestoneDefinition,
            3
        )
        && table_matches!(
            connection,
            world,
            activities,
            "SELECT hash, name, description, activity_type_hash, light_level, definition_version FROM activity_definitions ORDER BY hash",
            ActivityDefinition,
            3,
            4
        )
        && table_matches!(
            connection,
            world,
            modifiers,
            "SELECT hash, name, description, definition_version FROM activity_modifier_definitions ORDER BY hash",
            ActivityModifierDefinition
        )
        && table_matches!(
            connection,
            world,
            activity_types,
            "SELECT hash, name, description, definition_version FROM activity_type_definitions ORDER BY hash",
            ActivityTypeDefinition
        )
        && table_matches!(
            connection,
            world,
            objectives,
            "SELECT hash, progress_description, completion_value, definition_version FROM objective_definitions ORDER BY hash",
            ObjectiveDefinition
        ))
}
