use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use std::path::Path;
use store_core::{Operation, OperationLog, OperationState};

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

    /// Mark every operation left mid-flight by a previous run as cancelled.
    ///
    /// Nothing survives the process that was driving it: the child was killed
    /// with the backend, so an operation still recorded as Downloading or
    /// Installing at startup is a corpse. Left alone it shows up in Activity as
    /// a live install with a progress bar that never moves and a Cancel button
    /// that cancels nothing. Returns how many were reaped.
    pub fn reap_orphaned_operations(&self) -> Result<usize> {
        let mut reaped = 0;
        for mut op in self.list_operations()? {
            let terminal = matches!(
                op.state,
                OperationState::Succeeded | OperationState::Failed | OperationState::Cancelled
            );
            if terminal {
                continue;
            }
            op.state = OperationState::Cancelled;
            op.message = "interrupted by a backend restart".to_string();
            op.updated_at = Utc::now();
            self.upsert_operation(&op)?;
            reaped += 1;
        }
        Ok(reaped)
    }

    /// Delete one operation and the logs belonging to it.
    ///
    /// Only terminal operations can go: deleting the row for something still
    /// running would orphan the child process behind it, leaving work in flight
    /// that nothing on screen accounts for. Returns false when the id is
    /// unknown or the operation is still live.
    pub fn delete_operation(&self, id: &str) -> Result<bool> {
        let terminal = match self.get_operation(id)? {
            Some(op) => matches!(
                op.state,
                OperationState::Succeeded | OperationState::Failed | OperationState::Cancelled
            ),
            None => return Ok(false),
        };
        if !terminal {
            return Ok(false);
        }
        self.conn
            .execute("DELETE FROM operation_logs WHERE operation_id = ?1", [id])?;
        self.conn
            .execute("DELETE FROM operations WHERE id = ?1", [id])?;
        Ok(true)
    }

    /// Delete every operation sitting in one of `states`, and its logs.
    ///
    /// Non-terminal states are ignored rather than rejected, so a caller asking
    /// to clear "everything finished" cannot accidentally take a running
    /// install with it. Returns how many rows went.
    pub fn delete_operations_in_states(&self, states: &[OperationState]) -> Result<usize> {
        let mut removed = 0;
        for op in self.list_operations()? {
            let terminal = matches!(
                op.state,
                OperationState::Succeeded | OperationState::Failed | OperationState::Cancelled
            );
            if !terminal || !states.contains(&op.state) {
                continue;
            }
            if self.delete_operation(&op.id)? {
                removed += 1;
            }
        }
        Ok(removed)
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
            package_id: Some("demo".to_string()),
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

    #[test]
    fn dismiss_takes_finished_rows_and_leaves_running_ones() {
        let path = std::env::temp_dir().join(format!(
            "thallium-store-dismiss-{}-{:?}.db",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = StoreDb::open(&path).unwrap();
        let now = Utc::now();
        let make = |id: &str, state: OperationState| Operation {
            id: id.to_string(),
            app_id: "app".to_string(),
            app_name: "App".to_string(),
            variant_id: "flatpak:app".to_string(),
            source: SourceKind::Flathub,
            action: OperationAction::Install,
            package_id: Some("demo".to_string()),
            state,
            percent: 0,
            message: String::new(),
            created_at: now,
            updated_at: now,
        };

        db.upsert_operation(&make("live", OperationState::Downloading))
            .unwrap();
        db.upsert_operation(&make("bad", OperationState::Failed))
            .unwrap();
        db.upsert_operation(&make("good", OperationState::Succeeded))
            .unwrap();

        // A running operation still has a child process behind it.
        assert!(!db.delete_operation("live").unwrap());
        assert!(!db.delete_operation("nonexistent").unwrap());
        assert!(db.delete_operation("good").unwrap());

        // "Clear all errors" takes the failures and nothing else.
        assert_eq!(
            db.delete_operations_in_states(&[OperationState::Failed])
                .unwrap(),
            1
        );
        let left = db.list_operations().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, "live");
        let _ = std::fs::remove_file(path);
    }
}
