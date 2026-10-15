use super::{SqliteStore, storage_failure};
use destiny_core::wishlist::{
    SourcedWishlistRoll, WishlistDocument, WishlistRepository, WishlistRoll, WishlistSourceRecord,
};
use destiny_core::{AppError, Timestamp};
use rusqlite::{Transaction, params};
use std::collections::HashMap;

fn timestamp(text: String) -> Result<Timestamp, AppError> {
    text.parse()
        .map(Timestamp::new)
        .map_err(|_| storage_failure())
}

fn perks_text(perks: &[u32]) -> String {
    perks
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_perks(text: &str) -> Result<Vec<u32>, AppError> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|perk| perk.parse().map_err(|_| storage_failure()))
        .collect()
}

/// Interns notes and section titles, which repeat across thousands of rolls,
/// within one publication.
struct TextIds<'a> {
    transaction: &'a Transaction<'a>,
    ids: HashMap<String, i64>,
}

impl TextIds<'_> {
    fn id(&mut self, text: Option<&str>) -> Result<Option<i64>, AppError> {
        let Some(text) = text else {
            return Ok(None);
        };
        if let Some(id) = self.ids.get(text) {
            return Ok(Some(*id));
        }
        self.transaction
            .execute("INSERT INTO wishlist_texts(text) VALUES (?1)", [text])
            .map_err(|_| storage_failure())?;
        let id = self.transaction.last_insert_rowid();
        self.ids.insert(text.to_owned(), id);
        Ok(Some(id))
    }
}

fn delete_orphan_texts(transaction: &Transaction<'_>) -> Result<(), AppError> {
    transaction
        .execute(
            "DELETE FROM wishlist_texts WHERE id NOT IN (
                SELECT notes_id FROM wishlist_rolls WHERE notes_id IS NOT NULL
                UNION SELECT section_id FROM wishlist_rolls WHERE section_id IS NOT NULL)",
            [],
        )
        .map(|_| ())
        .map_err(|_| storage_failure())
}

impl WishlistRepository for SqliteStore {
    fn wishlist_sources(&self) -> Result<Vec<WishlistSourceRecord>, AppError> {
        let mut statement = self
            .connection()
            .prepare(
                "SELECT key,title,description,etag,fingerprint,checked_at,updated_at,
                    (SELECT COUNT(*) FROM wishlist_rolls WHERE source=key),skipped
                 FROM wishlist_sources ORDER BY position,key",
            )
            .map_err(|_| storage_failure())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            })
            .map_err(|_| storage_failure())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| storage_failure())?;
        rows.into_iter()
            .map(
                |(key, title, description, etag, fingerprint, checked, updated, rolls, skipped)| {
                    Ok(WishlistSourceRecord {
                        key,
                        title,
                        description,
                        etag,
                        fingerprint,
                        checked_at: timestamp(checked)?,
                        updated_at: timestamp(updated)?,
                        rolls: usize::try_from(rolls).map_err(|_| storage_failure())?,
                        skipped: usize::try_from(skipped).map_err(|_| storage_failure())?,
                    })
                },
            )
            .collect()
    }

    fn publish_wishlist(
        &mut self,
        key: &str,
        document: &WishlistDocument,
        etag: Option<&str>,
        fingerprint: &str,
        checked_at: Timestamp,
    ) -> Result<(), AppError> {
        let transaction = self
            .connection_mut()
            .transaction()
            .map_err(|_| storage_failure())?;
        transaction
            .execute("DELETE FROM wishlist_rolls WHERE source=?1", [key])
            .map_err(|_| storage_failure())?;
        let at = checked_at.seconds().to_string();
        transaction
            .execute(
                "INSERT INTO wishlist_sources(key,position,title,description,etag,fingerprint,checked_at,updated_at,skipped)
                 VALUES (?1,(SELECT COALESCE(MAX(position)+1,0) FROM wishlist_sources),?2,?3,?4,?5,?6,?6,?7)
                 ON CONFLICT(key) DO UPDATE SET title=excluded.title,description=excluded.description,
                    etag=excluded.etag,fingerprint=excluded.fingerprint,checked_at=excluded.checked_at,
                    updated_at=excluded.updated_at,skipped=excluded.skipped",
                params![
                    key,
                    document.title,
                    document.description,
                    etag,
                    fingerprint,
                    at,
                    i64::try_from(document.skipped).map_err(|_| storage_failure())?
                ],
            )
            .map_err(|_| storage_failure())?;
        {
            let mut texts = TextIds {
                transaction: &transaction,
                ids: HashMap::new(),
            };
            let mut insert = transaction
                .prepare(
                    "INSERT INTO wishlist_rolls(source,ordinal,item_hash,trash,perks,notes_id,section_id)
                     VALUES (?1,?2,?3,?4,?5,?6,?7)",
                )
                .map_err(|_| storage_failure())?;
            for roll in &document.rolls {
                let notes = texts.id(roll.notes.as_deref())?;
                let section = texts.id(roll.section.as_deref())?;
                insert
                    .execute(params![
                        key,
                        roll.ordinal,
                        roll.item_hash,
                        roll.trash,
                        perks_text(&roll.perks),
                        notes,
                        section
                    ])
                    .map_err(|_| storage_failure())?;
            }
        }
        delete_orphan_texts(&transaction)?;
        transaction.commit().map_err(|_| storage_failure())
    }

    fn mark_wishlist_checked(
        &mut self,
        key: &str,
        etag: Option<&str>,
        checked_at: Timestamp,
    ) -> Result<(), AppError> {
        let changed = self
            .connection_mut()
            .execute(
                "UPDATE wishlist_sources SET etag=?2,checked_at=?3 WHERE key=?1",
                params![key, etag, checked_at.seconds().to_string()],
            )
            .map_err(|_| storage_failure())?;
        if changed == 1 {
            Ok(())
        } else {
            Err(storage_failure())
        }
    }

    fn retain_wishlist_sources(&mut self, keys: &[String]) -> Result<(), AppError> {
        let transaction = self
            .connection_mut()
            .transaction()
            .map_err(|_| storage_failure())?;
        let stored: Vec<String> = transaction
            .prepare("SELECT key FROM wishlist_sources")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| row.get(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(|_| storage_failure())?;
        for key in stored.iter().filter(|key| !keys.contains(key)) {
            transaction
                .execute("DELETE FROM wishlist_rolls WHERE source=?1", [key])
                .and_then(|_| {
                    transaction.execute("DELETE FROM wishlist_sources WHERE key=?1", [key])
                })
                .map_err(|_| storage_failure())?;
        }
        for (position, key) in keys.iter().enumerate() {
            transaction
                .execute(
                    "UPDATE wishlist_sources SET position=?2 WHERE key=?1",
                    params![key, i64::try_from(position).map_err(|_| storage_failure())?],
                )
                .map_err(|_| storage_failure())?;
        }
        delete_orphan_texts(&transaction)?;
        transaction.commit().map_err(|_| storage_failure())
    }

    fn wishlist_rolls(&self, item_hash: u32) -> Result<Vec<SourcedWishlistRoll>, AppError> {
        let mut statement = self
            .connection()
            .prepare(
                "SELECT r.source,r.ordinal,r.item_hash,r.trash,r.perks,n.text,s.text
                 FROM wishlist_rolls r
                 JOIN wishlist_sources src ON src.key=r.source
                 LEFT JOIN wishlist_texts n ON n.id=r.notes_id
                 LEFT JOIN wishlist_texts s ON s.id=r.section_id
                 WHERE r.item_hash=?1 OR r.item_hash IS NULL
                 ORDER BY src.position,src.key,r.ordinal",
            )
            .map_err(|_| storage_failure())?;
        let rows = statement
            .query_map([item_hash], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, Option<u32>>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(|_| storage_failure())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| storage_failure())?;
        rows.into_iter()
            .map(
                |(source, ordinal, item_hash, trash, perks, notes, section)| {
                    Ok(SourcedWishlistRoll {
                        source,
                        roll: WishlistRoll {
                            ordinal,
                            item_hash,
                            trash,
                            perks: parse_perks(&perks)?,
                            notes,
                            section,
                        },
                    })
                },
            )
            .collect()
    }
}
