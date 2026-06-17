//! Browsing history backed by SQLite on a dedicated writer thread.
//!
//! SQLite serializes writes, so a single owner thread is the correct model (see
//! design D3): the main loop never blocks on disk. [`History`] is a handle that
//! forwards record requests over a channel; the worker thread owns the
//! connection. Reads (for completion) are added with the completion subsystem.

use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

enum Request {
    Record { url: String, title: String },
    Shutdown,
}

/// Handle to the history writer thread.
pub struct History {
    tx: Option<Sender<Request>>,
    handle: Option<JoinHandle<()>>,
}

impl History {
    /// Open (or create) the history database at `path` and start the writer thread.
    pub fn open(path: &Path) -> History {
        let path = path.to_path_buf();
        let (tx, rx) = mpsc::channel::<Request>();
        let handle = thread::spawn(move || {
            let conn = match init_db(&path) {
                Ok(conn) => conn,
                Err(e) => {
                    eprintln!("[qbrsh] history: cannot open {}: {e}", path.display());
                    return;
                }
            };
            for req in rx {
                match req {
                    Request::Record { url, title } => {
                        if let Err(e) = record_visit(&conn, &url, &title) {
                            eprintln!("[qbrsh] history: record failed: {e}");
                        }
                    }
                    Request::Shutdown => break,
                }
            }
        });
        History {
            tx: Some(tx),
            handle: Some(handle),
        }
    }

    /// Record a visit to `url`. Non-blocking; the write happens on the worker.
    pub fn record(&self, url: &str, title: &str) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Request::Record {
                url: url.to_string(),
                title: title.to_string(),
            });
        }
    }
}

impl Drop for History {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Request::Shutdown);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn init_db(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS history (
            url TEXT NOT NULL,
            title TEXT NOT NULL DEFAULT '',
            visit_count INTEGER NOT NULL DEFAULT 1,
            last_visit INTEGER NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_history_url ON history(url);
        CREATE INDEX IF NOT EXISTS idx_history_last_visit ON history(last_visit);",
    )?;
    Ok(conn)
}

fn record_visit(conn: &Connection, url: &str, title: &str) -> rusqlite::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let updated = conn.execute(
        "UPDATE history
         SET visit_count = visit_count + 1,
             last_visit = ?1,
             title = CASE WHEN ?2 <> '' THEN ?2 ELSE title END
         WHERE url = ?3",
        params![now, title, url],
    )?;
    if updated == 0 {
        conn.execute(
            "INSERT INTO history (url, title, visit_count, last_visit) VALUES (?1, ?2, 1, ?3)",
            params![url, title, now],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_dedups_by_url() {
        let conn = init_db(Path::new(":memory:")).unwrap();
        record_visit(&conn, "https://a.test", "A").unwrap();
        record_visit(&conn, "https://a.test", "A2").unwrap();
        record_visit(&conn, "https://b.test", "B").unwrap();

        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2);

        let (count, title): (i64, String) = conn
            .query_row(
                "SELECT visit_count, title FROM history WHERE url = 'https://a.test'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(title, "A2");
    }
}
