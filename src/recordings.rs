use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Clone, serde::Serialize)]
pub struct RecordingEntry {
    pub id: String,
    pub session_id: String,
    pub session_name: String,
    pub tmux_name: String,
    pub work_dir: String,
    #[serde(rename = "name")]
    pub file_name: String,
    #[serde(rename = "path")]
    pub file_path: String,
    pub status: String,
    pub started_at: u64,
    pub stopped_at: Option<u64>,
    pub size: u64,
    pub modified: u64,
}

pub struct RecordingStore {
    conn: Mutex<Connection>,
}

pub struct StartRecording<'a> {
    pub session_id: &'a str,
    pub session_name: &'a str,
    pub tmux_name: &'a str,
    pub work_dir: &'a str,
    pub file_path: &'a Path,
}

impl RecordingStore {
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(data_dir)
            .map_err(|e| format!("Failed to create data dir: {}", e))?;

        let db_path = data_dir.join("recordings.db");
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open recordings database: {}", e))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS context_recordings (
                id           TEXT PRIMARY KEY,
                session_id   TEXT NOT NULL DEFAULT '',
                session_name TEXT NOT NULL DEFAULT '',
                tmux_name    TEXT NOT NULL DEFAULT '',
                work_dir     TEXT NOT NULL,
                file_name    TEXT NOT NULL,
                file_path    TEXT NOT NULL UNIQUE,
                status       TEXT NOT NULL,
                started_at   INTEGER NOT NULL,
                stopped_at   INTEGER,
                size         INTEGER NOT NULL DEFAULT 0,
                modified     INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_recordings_workdir ON context_recordings(work_dir);
            CREATE INDEX IF NOT EXISTS idx_recordings_modified ON context_recordings(modified DESC);
            CREATE INDEX IF NOT EXISTS idx_recordings_status ON context_recordings(status);",
        )
        .map_err(|e| format!("Failed to create recordings table: {}", e))?;

        let now = epoch_secs();
        conn.execute(
            "UPDATE context_recordings
             SET status = 'interrupted', stopped_at = COALESCE(stopped_at, ?1)
             WHERE status = 'recording'",
            params![now as i64],
        )
        .map_err(|e| format!("Failed to mark interrupted recordings: {}", e))?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn start(&self, req: StartRecording<'_>) -> Result<RecordingEntry, String> {
        let now = epoch_secs();
        let (size, modified) = file_meta(req.file_path).unwrap_or((0, now));
        let file_name = file_name(req.file_path)?;
        let file_path = path_string(req.file_path);
        let id = short_uuid("rec");

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO context_recordings
             (id, session_id, session_name, tmux_name, work_dir, file_name, file_path, status, started_at, stopped_at, size, modified)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'recording', ?8, NULL, ?9, ?10)",
            params![
                id,
                req.session_id,
                req.session_name,
                req.tmux_name,
                req.work_dir,
                file_name,
                file_path,
                now as i64,
                size as i64,
                modified as i64,
            ],
        )
        .map_err(|e| format!("Failed to insert recording: {}", e))?;

        Ok(RecordingEntry {
            id,
            session_id: req.session_id.to_string(),
            session_name: req.session_name.to_string(),
            tmux_name: req.tmux_name.to_string(),
            work_dir: req.work_dir.to_string(),
            file_name,
            file_path,
            status: "recording".to_string(),
            started_at: now,
            stopped_at: None,
            size,
            modified,
        })
    }

    pub fn finish_by_path(&self, file_path: &Path) -> Result<(), String> {
        let now = epoch_secs();
        let (size, modified) = file_meta(file_path).unwrap_or((0, now));
        let path = path_string(file_path);

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE context_recordings
             SET status = 'completed', stopped_at = ?1, size = ?2, modified = ?3
             WHERE file_path = ?4",
            params![now as i64, size as i64, modified as i64, path],
        )
        .map_err(|e| format!("Failed to finish recording: {}", e))?;

        Ok(())
    }

    pub fn sync_and_list(
        &self,
        work_dir: &str,
        context_dir: &Path,
    ) -> Result<Vec<RecordingEntry>, String> {
        let mut seen = Vec::new();

        if context_dir.exists() {
            let entries = std::fs::read_dir(context_dir)
                .map_err(|e| format!("Cannot read context dir: {}", e))?;
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    seen.push(path_string(&path));
                    self.upsert_existing_file(work_dir, &path)?;
                }
            }
        }

        self.prune_missing(work_dir, &seen)?;
        self.list_by_work_dir(work_dir)
    }

    pub fn delete_by_names(&self, work_dir: &str, names: &[String]) -> Result<usize, String> {
        let conn = self.conn.lock().unwrap();
        let mut deleted = 0usize;
        for name in names {
            let rows = conn
                .execute(
                    "DELETE FROM context_recordings WHERE work_dir = ?1 AND file_name = ?2",
                    params![work_dir, name],
                )
                .map_err(|e| format!("Delete recording error: {}", e))?;
            deleted += rows;
        }
        Ok(deleted)
    }

    fn list_by_work_dir(&self, work_dir: &str) -> Result<Vec<RecordingEntry>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, session_name, tmux_name, work_dir, file_name, file_path,
                        status, started_at, stopped_at, size, modified
                 FROM context_recordings
                 WHERE work_dir = ?1
                 ORDER BY modified DESC, started_at DESC",
            )
            .map_err(|e| format!("Query recordings error: {}", e))?;

        let rows = stmt
            .query_map(params![work_dir], row_to_entry)
            .map_err(|e| format!("Query recordings error: {}", e))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(rows)
    }

    fn upsert_existing_file(&self, work_dir: &str, file_path: &Path) -> Result<(), String> {
        let now = epoch_secs();
        let (size, modified) = file_meta(file_path).unwrap_or((0, now));
        let file_name = file_name(file_path)?;
        let path = path_string(file_path);

        let conn = self.conn.lock().unwrap();
        let existing: Option<(String, String, i64)> = conn
            .query_row(
                "SELECT id, status, started_at FROM context_recordings WHERE file_path = ?1",
                params![path],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|e| format!("Query recording error: {}", e))?;

        if let Some((id, status, started_at)) = existing {
            let next_status = if status == "recording" || status == "interrupted" {
                status
            } else {
                "completed".to_string()
            };
            conn.execute(
                "UPDATE context_recordings
                 SET work_dir = ?1, file_name = ?2, status = ?3, size = ?4, modified = ?5
                 WHERE id = ?6",
                params![
                    work_dir,
                    file_name,
                    next_status,
                    size as i64,
                    modified as i64,
                    id
                ],
            )
            .map_err(|e| format!("Update recording error: {}", e))?;

            if started_at <= 0 {
                conn.execute(
                    "UPDATE context_recordings SET started_at = ?1 WHERE id = ?2",
                    params![modified as i64, id],
                )
                .map_err(|e| format!("Update recording timestamp error: {}", e))?;
            }
        } else {
            let id = short_uuid("rec");
            conn.execute(
                "INSERT INTO context_recordings
                 (id, session_id, session_name, tmux_name, work_dir, file_name, file_path, status, started_at, stopped_at, size, modified)
                 VALUES (?1, '', '', '', ?2, ?3, ?4, 'completed', ?5, ?5, ?6, ?7)",
                params![
                    id,
                    work_dir,
                    file_name,
                    path,
                    modified as i64,
                    size as i64,
                    modified as i64,
                ],
            )
            .map_err(|e| format!("Insert recording error: {}", e))?;
        }

        Ok(())
    }

    fn prune_missing(&self, work_dir: &str, seen_paths: &[String]) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, file_path, status FROM context_recordings WHERE work_dir = ?1")
            .map_err(|e| format!("Query recordings error: {}", e))?;
        let rows: Vec<(String, String, String)> = stmt
            .query_map(params![work_dir], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|e| format!("Query recordings error: {}", e))?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);

        for (id, path, status) in rows {
            if status == "recording" {
                continue;
            }
            if !seen_paths.iter().any(|p| p == &path) {
                conn.execute("DELETE FROM context_recordings WHERE id = ?1", params![id])
                    .map_err(|e| format!("Prune recording error: {}", e))?;
            }
        }

        Ok(())
    }
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecordingEntry> {
    let stopped_at: Option<i64> = row.get(9)?;
    Ok(RecordingEntry {
        id: row.get(0)?,
        session_id: row.get(1)?,
        session_name: row.get(2)?,
        tmux_name: row.get(3)?,
        work_dir: row.get(4)?,
        file_name: row.get(5)?,
        file_path: row.get(6)?,
        status: row.get(7)?,
        started_at: i64_to_u64(row.get(8)?),
        stopped_at: stopped_at.map(i64_to_u64),
        size: i64_to_u64(row.get(10)?),
        modified: i64_to_u64(row.get(11)?),
    })
}

fn file_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| "Recording path has no filename".to_string())
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn file_meta(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or_else(epoch_secs);
    Some((meta.len(), modified))
}

fn short_uuid(prefix: &str) -> String {
    format!(
        "{}_{}",
        prefix,
        &uuid::Uuid::new_v4().to_string().replace('-', "")[..12]
    )
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn i64_to_u64(value: i64) -> u64 {
    if value < 0 {
        0
    } else {
        value as u64
    }
}
