//! Historical schema changes, each committed atomically before the next migration.
use crate::{Error, Result};
use agent_common::{types::TelemetryFrame, usage};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::HashSet;

pub const VERSION: i64 = 10;

pub fn migrate(db: &mut Connection) -> Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if !(0..=VERSION).contains(&version) {
        return Err(Error::SchemaVersion(version));
    }
    for next in version + 1..=VERSION {
        if next == 6 {
            rebuild_agents(db)?;
            continue;
        }
        let tx = db.transaction()?;
        match next {
            1 => tx.execute_batch(include_str!("sql/v1.sql"))?,
            2 => tx.execute_batch(
                "ALTER TABLE agents ADD COLUMN is_hidden INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE agents ADD COLUMN hidden_at INTEGER;
                CREATE INDEX IF NOT EXISTS idx_agents_hidden ON agents(is_hidden);",
            )?,
            3 => {
                tx.execute_batch("ALTER TABLE agents ADD COLUMN active_tool_started_at INTEGER;")?
            }
            4 => tx.execute_batch("ALTER TABLE agents ADD COLUMN active_subagents TEXT;")?,
            5 => tx.execute_batch(
                "ALTER TABLE agents ADD COLUMN model TEXT;
                ALTER TABLE agents ADD COLUMN context_window INTEGER;
                ALTER TABLE agents ADD COLUMN context_fullness REAL;",
            )?,
            7 => add_columns(
                &tx,
                &[
                    ("rate_limit_reset_at", "INTEGER"),
                    ("retry_after_seconds", "INTEGER"),
                ],
            )?,
            8 => add_columns(
                &tx,
                &[
                    ("has_unseen_completion", "INTEGER NOT NULL DEFAULT 0"),
                    ("completed_at", "INTEGER"),
                ],
            )?,
            9 => tx.execute_batch(
                "ALTER TABLE agents ADD COLUMN summary TEXT;
                ALTER TABLE agents ADD COLUMN summary_updated_at INTEGER;",
            )?,
            10 => {
                tx.execute_batch(include_str!("sql/usage.sql"))?;
                backfill_usage(&tx)?;
            }
            _ => unreachable!("every version in the bounded migration sequence is implemented"),
        }
        tx.pragma_update(None, "user_version", next)?;
        tx.commit()?;
    }
    Ok(())
}

fn add_columns(tx: &Transaction<'_>, columns: &[(&str, &str)]) -> Result<()> {
    let existing: HashSet<String> = tx
        .prepare("PRAGMA table_info(agents)")?
        .query_map([], |row| row.get(1))?
        .collect::<std::result::Result<_, _>>()?;
    // Both identifiers and declarations come exclusively from the static migration list.
    for &(name, declaration) in columns {
        if !existing.contains(name) {
            tx.execute_batch(&format!(
                "ALTER TABLE agents ADD COLUMN {name} {declaration}"
            ))?;
        }
    }
    Ok(())
}

fn rebuild_agents(db: &mut Connection) -> Result<()> {
    // SQLite requires disabling foreign keys before entering the rebuild transaction.
    db.pragma_update(None, "foreign_keys", false)?;
    let result = (|| {
        let tx = db.transaction()?;
        tx.execute_batch(include_str!("sql/v6.sql"))?;
        let violation = tx
            .query_row("PRAGMA foreign_key_check", [], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .optional()?;
        if let Some((table, rowid, parent)) = violation {
            return Err(Error::ForeignKey {
                table,
                parent,
                rowid,
            });
        }
        tx.pragma_update(None, "user_version", 6)?;
        tx.commit()?;
        Ok(())
    })();
    let restored = db.pragma_update(None, "foreign_keys", true);
    result?;
    restored?;
    Ok(())
}

pub(crate) fn record_usage(tx: &Transaction<'_>, frame: &TelemetryFrame) -> Result<()> {
    for observation in usage::extract(frame) {
        let evidence = observation.evidence;
        tx.execute("INSERT INTO usage_observations
            (agent_id,observation_id,subagent_id,kind,name,server,operation,observed_at,source,event_type,tool_call_id)
            VALUES(?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(agent_id,observation_id) DO NOTHING",
            params![frame.agent_id,observation.id,evidence.subagent_id,observation.kind,observation.name,
                observation.server,observation.operation,evidence.timestamp,evidence.source,evidence.event_type,evidence.tool_call_id])?;
    }
    Ok(())
}

fn backfill_usage(tx: &Transaction<'_>) -> Result<()> {
    let mut statement = tx.prepare(
        "SELECT te.agent_id,te.subagent_id,a.harness,te.event_type,te.timestamp,te.data_json
        FROM telemetry_events te JOIN agents a ON a.id=te.agent_id
        WHERE te.event_type IN ('tool_call','tool_use','turn_start','user_prompt') ORDER BY te.id",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let data: String = row.get(5)?;
        let harness: String = row.get(2)?;
        let frame = TelemetryFrame {
            agent_id: row.get(0)?,
            subagent_id: row.get(1)?,
            harness: serde_json::from_value(serde_json::Value::String(harness))?,
            event_type: row.get(3)?,
            timestamp: row.get(4)?,
            data: serde_json::from_str(&data)?,
            ..Default::default()
        };
        record_usage(tx, &frame)?;
    }
    Ok(())
}
