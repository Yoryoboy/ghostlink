use super::{SqliteStore, storage_failure};
use destiny_core::{AppError, ClarityInsight, ClarityMetadata, ClarityRepository, Timestamp};
use rusqlite::{OptionalExtension, params};

fn entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClarityInsight> {
    Ok(ClarityInsight {
        hash: row.get(0)?,
        name: row.get(1)?,
        kind: row.get(2)?,
        item_hash: row.get(3)?,
        item_name: row.get(4)?,
        text: row.get(5)?,
    })
}
impl ClarityRepository for SqliteStore {
    fn clarity_by_hash(&self, hash: u32) -> Result<Option<ClarityInsight>, AppError> {
        self.connection().query_row("SELECT hash,name,kind,item_hash,item_name,text FROM clarity_insights WHERE hash=?1", [hash], entry)
            .optional().map_err(|_| storage_failure())
    }
    fn clarity_by_name(&self, name: &str) -> Result<Vec<ClarityInsight>, AppError> {
        let mut statement = self.connection().prepare("SELECT hash,name,kind,item_hash,item_name,text FROM clarity_insights WHERE name=?1 ORDER BY hash").map_err(|_| storage_failure())?;
        statement
            .query_map([name], entry)
            .map_err(|_| storage_failure())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| storage_failure())
    }
    fn clarity_metadata(&self) -> Result<Option<ClarityMetadata>, AppError> {
        let raw: Option<(String, String, String, u32)> = self.connection().query_row(
            "SELECT version,checked_at,updated_at,(SELECT COUNT(*) FROM clarity_insights) FROM clarity_metadata WHERE id=1", [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
            .optional().map_err(|_| storage_failure())?;
        raw.map(|(version, checked, updated, entries)| {
            Ok(ClarityMetadata {
                version,
                checked_at: Timestamp::new(checked.parse().map_err(|_| storage_failure())?),
                updated_at: Timestamp::new(updated.parse().map_err(|_| storage_failure())?),
                entries: entries as usize,
            })
        })
        .transpose()
    }
    fn publish_clarity(
        &mut self,
        version: &str,
        entries: &[ClarityInsight],
        checked_at: Timestamp,
    ) -> Result<(), AppError> {
        let transaction = self
            .connection_mut()
            .transaction()
            .map_err(|_| storage_failure())?;
        transaction
            .execute("DELETE FROM clarity_insights", [])
            .map_err(|_| storage_failure())?;
        {
            let mut insert = transaction.prepare("INSERT INTO clarity_insights(hash,name,kind,item_hash,item_name,text) VALUES (?1,?2,?3,?4,?5,?6)").map_err(|_| storage_failure())?;
            for value in entries {
                insert
                    .execute(params![
                        value.hash,
                        value.name,
                        value.kind,
                        value.item_hash,
                        value.item_name,
                        value.text
                    ])
                    .map_err(|_| storage_failure())?;
            }
        }
        transaction.execute("INSERT INTO clarity_metadata(id,version,checked_at,updated_at) VALUES(1,?1,?2,?2) ON CONFLICT(id) DO UPDATE SET version=excluded.version,checked_at=excluded.checked_at,updated_at=excluded.updated_at", params![version, checked_at.seconds().to_string()]).map_err(|_| storage_failure())?;
        transaction.commit().map_err(|_| storage_failure())
    }
    fn mark_clarity_checked(&mut self, checked_at: Timestamp) -> Result<(), AppError> {
        self.connection_mut()
            .execute(
                "UPDATE clarity_metadata SET checked_at=?1 WHERE id=1",
                [checked_at.seconds().to_string()],
            )
            .map_err(|_| storage_failure())?;
        Ok(())
    }
}
