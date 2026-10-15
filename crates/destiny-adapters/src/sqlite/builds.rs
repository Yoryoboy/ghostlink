//! DIM-shaped payload plus indexed metadata; raw extra retains exact core text.
use super::{SqliteStore, parse_decimal, storage_failure};
use crate::dim_loadout::{DimLoadout, DimStatConstraint};
use destiny_core::{AppError, DestinyMembership, Timestamp, build::*, ports::BuildRepository};
use rusqlite::{OptionalExtension, params};

fn source_text(source: BuildSource) -> &'static str {
    match source {
        BuildSource::Import => "import",
        BuildSource::Equipped => "equipped",
        BuildSource::InGameLoadout => "ingame",
    }
}

fn read_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<BuildSummary> {
    let corrupt = || rusqlite::Error::InvalidQuery;
    let source: String = row.get(3)?;
    let created: String = row.get(4)?;
    let updated: String = row.get(5)?;
    Ok(BuildSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        class_type: row.get(2)?,
        source: match source.as_str() {
            "import" => BuildSource::Import,
            "equipped" => BuildSource::Equipped,
            "ingame" => BuildSource::InGameLoadout,
            _ => return Err(corrupt()),
        },
        created_at: Timestamp::new(parse_decimal(&created).map_err(|_| corrupt())?),
        updated_at: Timestamp::new(parse_decimal(&updated).map_err(|_| corrupt())?),
    })
}

fn decode(json: &str, extra: Option<String>) -> Result<BuildDefinition, AppError> {
    let dto: DimLoadout = serde_json::from_str(json).map_err(|_| storage_failure())?;
    // Import migration/clamping is not storage decoding. Restore exact modern
    // constraints (including independently valid crossing ranges) after decode.
    let constraints = dto
        .parameters
        .as_ref()
        .and_then(|p| p.stat_constraints.clone());
    let mut build: BuildDefinition = dto.try_into().map_err(|_| storage_failure())?;
    if let Some(p) = &mut build.parameters {
        p.extra = extra;
        p.stat_constraints = constraints
            .map(|cs| {
                cs.into_iter()
                    .map(|c| {
                        Ok(StatConstraint {
                            stat_hash: c.stat_hash,
                            min_stat: c
                                .min_stat
                                .map(u16::try_from)
                                .transpose()
                                .map_err(|_| storage_failure())?,
                            max_stat: c
                                .max_stat
                                .map(u16::try_from)
                                .transpose()
                                .map_err(|_| storage_failure())?,
                        })
                    })
                    .collect::<Result<_, AppError>>()
            })
            .transpose()?;
    } else if extra.is_some() {
        return Err(storage_failure());
    }
    build.validate().map_err(|_| storage_failure())?;
    Ok(build)
}

fn read_build(
    connection: &rusqlite::Connection,
    membership: &DestinyMembership,
    id: &str,
) -> Result<Option<StoredBuild>, AppError> {
    let row = connection.query_row("SELECT id,name,class_type,source,created_at,updated_at,normalized_build,parameter_extra FROM builds WHERE membership_type=?1 AND membership_id=?2 AND id=?3", params![membership.membership_type().value(), membership.membership_id().as_str(), id], |row| Ok((read_summary(row)?, row.get::<_, String>(6)?, row.get::<_, Option<String>>(7)?))).optional().map_err(|_| storage_failure())?;
    row.map(|(summary, json, extra)| {
        let definition = decode(&json, extra)?;
        if definition.id != summary.id
            || definition.name != summary.name
            || definition.class_type != summary.class_type
        {
            return Err(storage_failure());
        }
        Ok(StoredBuild {
            summary,
            definition,
        })
    })
    .transpose()
}

impl BuildRepository for SqliteStore {
    fn save_build(
        &mut self,
        membership: &DestinyMembership,
        build: &BuildDefinition,
        source: BuildSource,
        now: Timestamp,
    ) -> Result<StoredBuild, AppError> {
        build.validate()?;
        // Extra is adapter-interpreted text, so preserve its exact representation
        // separately, even for constructed values the offline codec can't export.
        let mut payload = build.clone();
        if let Some(p) = &mut payload.parameters {
            p.extra = None;
            p.stat_constraints = None;
        }
        let mut dto = DimLoadout::try_from(&payload)
            .map_err(|_| AppError::new(destiny_core::ErrorCategory::ProviderValidation))?;
        if let (Some(stored), Some(original)) = (&mut dto.parameters, &build.parameters) {
            stored.stat_constraints = original.stat_constraints.as_ref().map(|values| {
                values
                    .iter()
                    .map(|c| DimStatConstraint {
                        stat_hash: c.stat_hash,
                        min_stat: c.min_stat.map(i64::from),
                        max_stat: c.max_stat.map(i64::from),
                        min_tier: None,
                        max_tier: None,
                    })
                    .collect()
            });
        }
        let json = serde_json::to_string(&dto).map_err(|_| storage_failure())?;
        let extra = build.parameters.as_ref().and_then(|p| p.extra.as_deref());
        // Verify losslessness before any write, never silently normalize a payload.
        if decode(&json, extra.map(str::to_owned))? != *build {
            return Err(storage_failure());
        }
        let transaction = self
            .connection_mut()
            .transaction()
            .map_err(|_| storage_failure())?;
        transaction.execute("INSERT INTO builds(membership_type,membership_id,id,name,class_type,source,created_at,updated_at,normalized_build,parameter_extra) VALUES(?1,?2,?3,?4,?5,?6,?7,?7,?8,?9) ON CONFLICT(membership_type,membership_id,id) DO UPDATE SET name=excluded.name,class_type=excluded.class_type,source=excluded.source,updated_at=excluded.updated_at,normalized_build=excluded.normalized_build,parameter_extra=excluded.parameter_extra", params![membership.membership_type().value(), membership.membership_id().as_str(), build.id, build.name, build.class_type, source_text(source), now.seconds().to_string(), json, extra]).map_err(|_| storage_failure())?;
        let stored =
            read_build(&transaction, membership, &build.id)?.ok_or_else(storage_failure)?;
        transaction.commit().map_err(|_| storage_failure())?;
        Ok(stored)
    }

    fn list_builds(&self, membership: &DestinyMembership) -> Result<Vec<BuildSummary>, AppError> {
        let mut statement = self.connection().prepare("SELECT id,name,class_type,source,created_at,updated_at FROM builds WHERE membership_type=?1 AND membership_id=?2 ORDER BY id").map_err(|_| storage_failure())?;
        statement
            .query_map(
                params![
                    membership.membership_type().value(),
                    membership.membership_id().as_str()
                ],
                read_summary,
            )
            .map_err(|_| storage_failure())?
            .collect::<Result<_, _>>()
            .map_err(|_| storage_failure())
    }

    fn get_build(
        &self,
        membership: &DestinyMembership,
        id: &str,
    ) -> Result<Option<StoredBuild>, AppError> {
        read_build(self.connection(), membership, id)
    }

    fn delete_build(&mut self, membership: &DestinyMembership, id: &str) -> Result<bool, AppError> {
        self.connection()
            .execute(
                "DELETE FROM builds WHERE membership_type=?1 AND membership_id=?2 AND id=?3",
                params![
                    membership.membership_type().value(),
                    membership.membership_id().as_str(),
                    id
                ],
            )
            .map(|count| count != 0)
            .map_err(|_| storage_failure())
    }
}
