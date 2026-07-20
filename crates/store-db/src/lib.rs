use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;
use store_core::{Operation, OperationLog};

pub struct StoreDb {
    conn: Connection,
}

impl StoreDb {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS operations (
                id TEXT PRIMARY KEY,
                payload TEXT NOT NULL,
                state TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS operation_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                operation_id TEXT NOT NULL,
                payload TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            "#,
        )?;
        Ok(())
    }

    pub fn upsert_operation(&self, operation: &Operation) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO operations (id, payload, state, updated_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(id) DO UPDATE SET
                payload = excluded.payload,
                state = excluded.state,
                updated_at = excluded.updated_at
            "#,
            params![
                &operation.id,
                serde_json::to_string(operation)?,
                format!("{:?}", operation.state),
                operation.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_operations(&self) -> Result<Vec<Operation>> {
        let mut stmt = self
            .conn
            .prepare("SELECT payload FROM operations ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], |row| {
            let payload: String = row.get(0)?;
            Ok(payload)
        })?;

        let mut operations = Vec::new();
        for row in rows {
            operations.push(serde_json::from_str(&row?)?);
        }
        Ok(operations)
    }

    pub fn get_operation(&self, id: &str) -> Result<Option<Operation>> {
        let mut stmt = self
            .conn
            .prepare("SELECT payload FROM operations WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            let payload: String = row.get(0)?;
            Ok(Some(serde_json::from_str(&payload)?))
        } else {
            Ok(None)
        }
    }

    pub fn add_log(&self, log: &OperationLog) -> Result<()> {
        self.conn.execute(
            "INSERT INTO operation_logs (operation_id, payload, created_at) VALUES (?1, ?2, ?3)",
            params![
                &log.operation_id,
                serde_json::to_string(log)?,
                log.timestamp.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn logs(&self, operation_id: &str) -> Result<Vec<OperationLog>> {
        let mut stmt = self.conn.prepare(
            "SELECT payload FROM operation_logs WHERE operation_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![operation_id], |row| {
            let payload: String = row.get(0)?;
            Ok(payload)
        })?;

        let mut logs = Vec::new();
        for row in rows {
            logs.push(serde_json::from_str(&row?)?);
        }
        Ok(logs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use store_core::{OperationAction, OperationState, SourceKind};

    #[test]
    fn stores_operation_roundtrip() {
        let path =
            std::env::temp_dir().join(format!("thallium-store-test-{}.db", std::process::id()));
        let db = StoreDb::open(&path).unwrap();
        let now = Utc::now();
        let op = Operation {
            id: "op1".to_string(),
            app_id: "app".to_string(),
            app_name: "App".to_string(),
            variant_id: "flatpak:app".to_string(),
            source: SourceKind::Flathub,
            action: OperationAction::Install,
            state: OperationState::Pending,
            percent: 0,
            message: "queued".to_string(),
            created_at: now,
            updated_at: now,
        };

        db.upsert_operation(&op).unwrap();
        assert_eq!(db.list_operations().unwrap().len(), 1);
        let _ = std::fs::remove_file(path);
    }
}
