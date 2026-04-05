use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

const RING_BUFFER_SIZE: usize = 1000;
const ZSTD_LEVEL: i32 = 3;

// ── Log entry ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// Unix timestamp with ms precision
    pub ts: f64,
    /// Session id (first 8 chars)
    pub sid: String,
    /// Direction: "in", "out", "event"
    pub d: String,
    /// Data: base64 string for PTY, or JSON value for ACP
    pub data: serde_json::Value,
}

// ── Public interface ──

#[derive(Clone)]
pub struct Logger {
    tx: mpsc::Sender<LogEntry>,
    rings: Arc<Mutex<HashMap<String, VecDeque<LogEntry>>>>,
}

impl Logger {
    /// Start the logger. Returns None if log_dir is None (logging disabled).
    pub fn start(log_dir: Option<&str>) -> Option<Self> {
        let log_dir = log_dir?;
        let log_path = PathBuf::from(log_dir);
        fs::create_dir_all(&log_path).ok()?;

        let (tx, rx) = mpsc::channel::<LogEntry>(4096);
        let rings: Arc<Mutex<HashMap<String, VecDeque<LogEntry>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let rings2 = rings.clone();
        tokio::spawn(writer_task(rx, log_path, rings2));

        Some(Self { tx, rings })
    }

    /// Log a PTY input (user keystroke, base64 encoded)
    pub fn log_pty_input(&self, session_id: &str, b64_data: &str) {
        let entry = LogEntry {
            ts: now(),
            sid: short_id(session_id),
            d: "in".into(),
            data: serde_json::Value::String(b64_data.to_string()),
        };
        let _ = self.tx.try_send(entry);
    }

    /// Log a PTY output (terminal data, base64 encoded)
    pub fn log_pty_output(&self, session_id: &str, b64_data: &str) {
        let entry = LogEntry {
            ts: now(),
            sid: short_id(session_id),
            d: "out".into(),
            data: serde_json::Value::String(b64_data.to_string()),
        };
        let _ = self.tx.try_send(entry);
    }

    /// Log an ACP input (user prompt)
    pub fn log_acp_input(&self, session_id: &str, text: &str) {
        let entry = LogEntry {
            ts: now(),
            sid: short_id(session_id),
            d: "in".into(),
            data: serde_json::Value::String(text.to_string()),
        };
        let _ = self.tx.try_send(entry);
    }

    /// Log an ACP event (assistant output)
    pub fn log_acp_event(&self, session_id: &str, event: &serde_json::Value) {
        let entry = LogEntry {
            ts: now(),
            sid: short_id(session_id),
            d: "event".into(),
            data: event.clone(),
        };
        let _ = self.tx.try_send(entry);
    }

    /// Query recent logs for an active session from ring buffer
    pub fn recent_logs(&self, session_id: &str, limit: usize, offset: usize) -> Vec<LogEntry> {
        let sid = short_id(session_id);
        let rings = self.rings.lock().unwrap();
        if let Some(ring) = rings.get(&sid) {
            ring.iter()
                .skip(offset)
                .take(limit)
                .cloned()
                .collect()
        } else {
            vec![]
        }
    }

    /// Remove ring buffer for a closed session
    pub fn remove_session(&self, session_id: &str) {
        let sid = short_id(session_id);
        self.rings.lock().unwrap().remove(&sid);
    }
}

// ── Internals ──

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn short_id(id: &str) -> String {
    id[..8.min(id.len())].to_string()
}

/// Get current date as YYYY-MM-DD without chrono crate
fn chrono_free_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Simple days-since-epoch calculation
    let days = secs / 86400;
    let mut y = 1970i64;
    let mut remaining = days as i64;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let months_days = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m = 1;
    for &md in &months_days {
        if remaining < md {
            break;
        }
        remaining -= md;
        m += 1;
    }
    let d = remaining + 1;
    format!("{:04}-{:02}-{:02}", y, m, d)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

struct ZstdWriter {
    encoder: zstd::Encoder<'static, File>,
    current_date: String,
}

impl ZstdWriter {
    fn open(dir: &Path) -> std::io::Result<Self> {
        let date = chrono_free_date();
        let path = dir.join(format!("{}.jsonl.zst", date));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let encoder = zstd::Encoder::new(file, ZSTD_LEVEL)?;
        Ok(Self {
            encoder,
            current_date: date,
        })
    }

    fn write_entry(&mut self, dir: &Path, entry: &LogEntry) -> std::io::Result<()> {
        let today = chrono_free_date();
        if today != self.current_date {
            // Day rolled over — finish current file, open new one
            self.encoder.do_finish()?;
            let path = dir.join(format!("{}.jsonl.zst", today));
            let file = OpenOptions::new().create(true).append(true).open(&path)?;
            self.encoder = zstd::Encoder::new(file, ZSTD_LEVEL)?;
            self.current_date = today;
        }

        let mut line = serde_json::to_string(entry).unwrap();
        line.push('\n');
        self.encoder.write_all(line.as_bytes())?;
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.encoder.flush()
    }
}

async fn writer_task(
    mut rx: mpsc::Receiver<LogEntry>,
    log_dir: PathBuf,
    rings: Arc<Mutex<HashMap<String, VecDeque<LogEntry>>>>,
) {
    let mut writer = match ZstdWriter::open(&log_dir) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!("Failed to open log file: {}", e);
            return;
        }
    };

    let mut batch_count = 0u32;

    loop {
        // Drain available entries (up to 100) or wait for one
        let entry = match rx.recv().await {
            Some(e) => e,
            None => break,
        };

        // Push to ring buffer
        {
            let mut rings = rings.lock().unwrap();
            let ring = rings
                .entry(entry.sid.clone())
                .or_insert_with(|| VecDeque::with_capacity(RING_BUFFER_SIZE));
            if ring.len() >= RING_BUFFER_SIZE {
                ring.pop_front();
            }
            ring.push_back(entry.clone());
        }

        // Write to zstd file
        if let Err(e) = writer.write_entry(&log_dir, &entry) {
            tracing::warn!("Log write error: {}", e);
        }

        batch_count += 1;

        // Drain any pending entries without blocking
        while let Ok(entry) = rx.try_recv() {
            {
                let mut rings = rings.lock().unwrap();
                let ring = rings
                    .entry(entry.sid.clone())
                    .or_insert_with(|| VecDeque::with_capacity(RING_BUFFER_SIZE));
                if ring.len() >= RING_BUFFER_SIZE {
                    ring.pop_front();
                }
                ring.push_back(entry.clone());
            }
            if let Err(e) = writer.write_entry(&log_dir, &entry) {
                tracing::warn!("Log write error: {}", e);
            }
            batch_count += 1;
            if batch_count >= 100 {
                break;
            }
        }

        // Flush periodically
        if batch_count >= 100 {
            let _ = writer.flush();
            batch_count = 0;
        }
    }

    // Final flush
    let _ = writer.flush();
}
