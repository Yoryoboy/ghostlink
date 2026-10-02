//! World catalogs share the item catalog's transaction and version metadata.
use super::{SqliteStore, storage_failure};
use destiny_core::ports::WorldDefinitionRepository;
use destiny_core::*;
use rusqlite::{Connection, OptionalExtension, params};

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

    fn world_definition_counts(&self) -> Result<[usize; 3], AppError> {
        let mut counts = [0; 3];
        for (index, sql) in [
            "SELECT COUNT(*) FROM milestone_definitions",
            "SELECT COUNT(*) FROM activity_definitions",
            "SELECT COUNT(*) FROM activity_modifier_definitions",
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
    connection.execute_batch("DELETE FROM milestone_definitions; DELETE FROM activity_definitions; DELETE FROM activity_modifier_definitions;")
        .map_err(|_| storage_failure())
}

pub(super) fn replace_catalog(
    connection: &Connection,
    world: &WorldDefinitionsAcquisition,
) -> Result<(), AppError> {
    clear_catalog(connection)?;
    let version = world.version.as_str();
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
    Ok(table_matches!(
        connection,
        world,
        milestones,
        "SELECT hash, name, description, milestone_type, definition_version FROM milestone_definitions ORDER BY hash",
        MilestoneDefinition,
        3
    ) && table_matches!(
        connection,
        world,
        activities,
        "SELECT hash, name, description, activity_type_hash, light_level, definition_version FROM activity_definitions ORDER BY hash",
        ActivityDefinition,
        3,
        4
    ) && table_matches!(
        connection,
        world,
        modifiers,
        "SELECT hash, name, description, definition_version FROM activity_modifier_definitions ORDER BY hash",
        ActivityModifierDefinition
    ))
}
