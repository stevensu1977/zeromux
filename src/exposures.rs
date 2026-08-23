use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

/// An exposed service: a stable public slug that maps to a local port.
/// Slug shape is "<workspace_hash>-<port>" (e.g. "k7f2a9qx-3000") — the
/// hash is per-session-stable so re-exposing the same port yields the same
/// URL, and unguessable so the URL itself is a capability.
/// session_id for standalone tunnels (ssh-tunnel-style port forwards that
/// are not tied to any ZeroMux session; they share the workspace hash so
/// re-creating a tunnel for the same port yields the same URL).
pub const TUNNEL_SESSION: &str = "_tunnel";

#[derive(Debug, Clone, serde::Serialize)]
pub struct Exposure {
    pub slug: String,
    pub session_id: String,
    pub port: u16,
    pub owner_id: String,
    pub shareable: bool,
    pub created_at: String,
    #[serde(default)]
    pub name: String,
}

pub struct ExposureStore {
    conn: Mutex<Connection>,
}

impl ExposureStore {
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(data_dir)
            .map_err(|e| format!("Failed to create data dir: {}", e))?;
        let conn = Connection::open(data_dir.join("exposures.db"))
            .map_err(|e| format!("Failed to open exposures database: {}", e))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS workspace_hashes (
                session_id TEXT PRIMARY KEY,
                hash       TEXT UNIQUE NOT NULL
            );
            CREATE TABLE IF NOT EXISTS exposures (
                slug       TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                port       INTEGER NOT NULL,
                owner_id   TEXT NOT NULL,
                shareable  INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                UNIQUE(session_id, port)
            );
            CREATE INDEX IF NOT EXISTS idx_exposures_session ON exposures(session_id);",
        )
        .map_err(|e| format!("Failed to create exposures tables: {}", e))?;
        // Migration: name column for tunnels (ignore "duplicate column" error).
        let _ = conn.execute("ALTER TABLE exposures ADD COLUMN name TEXT NOT NULL DEFAULT ''", []);
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Stable per-session hash, generated on first use.
    fn workspace_hash(&self, conn: &Connection, session_id: &str) -> Result<String, String> {
        if let Some(h) = conn
            .query_row(
                "SELECT hash FROM workspace_hashes WHERE session_id = ?1",
                params![session_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
        {
            return Ok(h);
        }
        // 8 lowercase base36 chars ≈ 41 bits — unguessable at web scale.
        let hash: String = (0..8)
            .map(|_| {
                let n = rand::random::<u8>() % 36;
                if n < 10 {
                    (b'0' + n) as char
                } else {
                    (b'a' + n - 10) as char
                }
            })
            .collect();
        conn.execute(
            "INSERT INTO workspace_hashes (session_id, hash) VALUES (?1, ?2)",
            params![session_id, hash],
        )
        .map_err(|e| e.to_string())?;
        Ok(hash)
    }

    /// Create (or return the existing) exposure for a session port.
    pub fn expose(
        &self,
        session_id: &str,
        port: u16,
        owner_id: &str,
    ) -> Result<Exposure, String> {
        let conn = self.conn.lock().unwrap();
        if let Some(e) = self.lookup_by_session_port(&conn, session_id, port)? {
            return Ok(e);
        }
        let hash = self.workspace_hash(&conn, session_id)?;
        let slug = format!("{}-{}", hash, port);
        let created_at = chrono_now();
        conn.execute(
            "INSERT INTO exposures (slug, session_id, port, owner_id, shareable, created_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5)",
            params![slug, session_id, port, owner_id, created_at],
        )
        .map_err(|e| e.to_string())?;
        Ok(Exposure {
            slug,
            session_id: session_id.to_string(),
            port,
            owner_id: owner_id.to_string(),
            shareable: false,
            created_at,
            name: String::new(),
        })
    }

    /// Create (or return) a standalone tunnel: an exposure not tied to any
    /// session, identified by name, pointing at a local port.
    pub fn create_tunnel(
        &self,
        port: u16,
        name: &str,
        owner_id: &str,
    ) -> Result<Exposure, String> {
        let conn = self.conn.lock().unwrap();
        if let Some(mut e) = self.lookup_by_session_port(&conn, TUNNEL_SESSION, port)? {
            if !name.is_empty() && e.name != name {
                conn.execute(
                    "UPDATE exposures SET name = ?2 WHERE slug = ?1",
                    params![e.slug, name],
                )
                .map_err(|e| e.to_string())?;
                e.name = name.to_string();
            }
            return Ok(e);
        }
        let hash = self.workspace_hash(&conn, TUNNEL_SESSION)?;
        let slug = format!("{}-{}", hash, port);
        let created_at = chrono_now();
        conn.execute(
            "INSERT INTO exposures (slug, session_id, port, owner_id, shareable, created_at, name)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)",
            params![slug, TUNNEL_SESSION, port, owner_id, created_at, name],
        )
        .map_err(|e| e.to_string())?;
        Ok(Exposure {
            slug,
            session_id: TUNNEL_SESSION.to_string(),
            port,
            owner_id: owner_id.to_string(),
            shareable: false,
            created_at,
            name: name.to_string(),
        })
    }

    pub fn lookup(&self, slug: &str) -> Option<Exposure> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT slug, session_id, port, owner_id, shareable, created_at, name
             FROM exposures WHERE slug = ?1",
            params![slug],
            row_to_exposure,
        )
        .optional()
        .ok()
        .flatten()
    }

    fn lookup_by_session_port(
        &self,
        conn: &Connection,
        session_id: &str,
        port: u16,
    ) -> Result<Option<Exposure>, String> {
        conn.query_row(
            "SELECT slug, session_id, port, owner_id, shareable, created_at, name
             FROM exposures WHERE session_id = ?1 AND port = ?2",
            params![session_id, port],
            row_to_exposure,
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    pub fn list_for_session(&self, session_id: &str) -> Vec<Exposure> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT slug, session_id, port, owner_id, shareable, created_at, name
             FROM exposures WHERE session_id = ?1",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![session_id], row_to_exposure)
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    pub fn set_shareable(&self, slug: &str, shareable: bool) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE exposures SET shareable = ?2 WHERE slug = ?1",
            params![slug, shareable as i64],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
    }

    pub fn remove(&self, slug: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM exposures WHERE slug = ?1", params![slug])
            .map(|n| n > 0)
            .unwrap_or(false)
    }
}

fn row_to_exposure(row: &rusqlite::Row) -> rusqlite::Result<Exposure> {
    Ok(Exposure {
        slug: row.get(0)?,
        session_id: row.get(1)?,
        port: row.get::<_, i64>(2)? as u16,
        owner_id: row.get(3)?,
        shareable: row.get::<_, i64>(4)? != 0,
        created_at: row.get(5)?,
        name: row.get(6).unwrap_or_default(),
    })
}

fn chrono_now() -> String {
    // RFC3339-ish UTC timestamp without pulling in chrono.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = now / 86400;
    let (y, m, d) = civil_from_days(days as i64);
    let secs = now % 86400;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// Days-since-epoch → (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
