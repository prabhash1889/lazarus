//! Synchronous SQLite persistence for the Phase 2.1 Host daemon core.
//!
//! This module owns everything durable about Host startup storage: opening or
//! creating the database file, enabling WAL + foreign keys + a busy timeout,
//! running explicit transactional migrations tracked in their own ledger
//! table, and reading/writing simple durable runtime metadata.
//!
//! Design rules taken from the plan:
//!
//! - Schema migrations are tracked independently from RPC method versions and
//!   package semver (plan section 9.4): the only version currency here is the
//!   `migrations` ledger table, never `env!("CARGO_PKG_VERSION")`.
//! - Every state transition is transactional (plan section 8): each migration
//!   runs inside one transaction that also records it in the ledger, so a
//!   crash mid-migration leaves the database exactly as before.
//! - The store is deliberately synchronous: startup recovery runs once, off
//!   the request path, before the async server accepts traffic.

use std::fmt;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

/// Highest migration version this binary knows how to reach. A database from
/// a newer Host must refuse to open rather than be silently misread.
pub const CURRENT_SCHEMA_VERSION: i64 = 2;

/// How long a writer waits for a competing local writer before giving up.
const BUSY_TIMEOUT_MS: u64 = 5_000;

/// Per-process durable replay budget. The supervisor keeps a tighter hot
/// spool; SQLite retains a larger restart-safe tail without growing forever.
pub const PROCESS_OUTPUT_CAP_BYTES: u64 = 8 * 1024 * 1024;

/// One ordered, immutable schema step. `statements` runs inside a single
/// transaction together with the ledger row that records it; never edit an
/// already-shipped migration, append a new one instead.
#[derive(Clone, Copy)]
pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    /// SQL executed verbatim before the ledger row is written, atomically.
    pub statements: &'static [&'static str],
}

/// The shipped migration chain. The `migrations` ledger table itself is
/// bootstrapped outside this list (it must exist before it can be written
/// to); these are the tracked, replayable steps above that floor.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "bootstrap_core_tables",
        statements: &["CREATE TABLE IF NOT EXISTS runtime_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at_utc TEXT NOT NULL
        ) STRICT"],
    },
    Migration {
        version: 2,
        name: "add_supervised_processes",
        statements: &[
            "CREATE TABLE IF NOT EXISTS supervised_processes (
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL CHECK (status IN ('STARTING', 'RUNNING', 'EXITED', 'STOPPED', 'INTERRUPTED')),
                program TEXT NOT NULL,
                args_json TEXT NOT NULL,
                cwd TEXT,
                run_mode TEXT NOT NULL,
                pid INTEGER,
                started_at_utc TEXT NOT NULL,
                exited_at_utc TEXT,
                exit_code INTEGER,
                duration_ms INTEGER CHECK (duration_ms >= 0),
                stdout_bytes INTEGER NOT NULL DEFAULT 0 CHECK (stdout_bytes >= 0),
                stderr_bytes INTEGER NOT NULL DEFAULT 0 CHECK (stderr_bytes >= 0),
                cpu_ms INTEGER CHECK (cpu_ms >= 0),
                peak_memory_bytes INTEGER CHECK (peak_memory_bytes >= 0),
                dropped_output_bytes INTEGER NOT NULL DEFAULT 0 CHECK (dropped_output_bytes >= 0),
                next_output_offset INTEGER NOT NULL DEFAULT 0 CHECK (next_output_offset >= 0)
            ) STRICT",
            "CREATE TABLE IF NOT EXISTS process_output_frames (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                process_id TEXT NOT NULL REFERENCES supervised_processes(id),
                seq INTEGER NOT NULL,
                stream TEXT NOT NULL,
                payload BLOB NOT NULL,
                created_at_utc TEXT NOT NULL,
                UNIQUE(process_id, seq)
            ) STRICT",
            "CREATE TABLE IF NOT EXISTS process_interruptions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                process_id TEXT NOT NULL REFERENCES supervised_processes(id),
                detected_at_utc TEXT NOT NULL,
                reason TEXT NOT NULL
            ) STRICT",
        ],
    },
];

/// Why a persistence operation failed. Every variant is safe to log: none of
/// them embeds caller data such as token material.
#[derive(Debug)]
pub enum PersistenceError {
    /// The database file could not be opened or created.
    Open(rusqlite::Error),
    /// The existing database failed its integrity check.
    Corrupt(String),
    /// A plain statement failed outside a migration.
    Sqlite {
        context: &'static str,
        source: rusqlite::Error,
    },
    /// A migration step failed and was rolled back; the database is left at
    /// the previous schema version.
    MigrationFailed {
        version: i64,
        source: rusqlite::Error,
    },
    /// The database was written by a newer Host whose migrations this binary
    /// does not know; refuse to touch it instead of guessing.
    DatabaseTooNew { on_disk: i64, supported: i64 },
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(source) => write!(f, "opening database: {source}"),
            Self::Corrupt(report) => write!(f, "database failed integrity check: {report}"),
            Self::Sqlite { context, source } => write!(f, "{context}: {source}"),
            Self::MigrationFailed { version, source } => {
                write!(f, "migration {version} failed and rolled back: {source}")
            }
            Self::DatabaseTooNew { on_disk, supported } => write!(
                f,
                "database schema version {on_disk} is newer than the highest version this Host supports ({supported}); refusing to open it"
            ),
        }
    }
}

impl std::error::Error for PersistenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Open(source)
            | Self::Sqlite { source, .. }
            | Self::MigrationFailed { source, .. } => Some(source),
            Self::Corrupt(_) | Self::DatabaseTooNew { .. } => None,
        }
    }
}

/// A handle over one SQLite database. Cheap to hold; all operations are
/// synchronous and suitable for the single-threaded startup path.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

/// Counters persisted with a supervised-process lifecycle transition.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredResourceCounters {
    pub duration_ms: Option<u64>,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub cpu_ms: Option<u64>,
    pub peak_memory_bytes: Option<u64>,
}

/// One durable supervised-process row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredProcess {
    pub id: String,
    pub status: String,
    pub started_at: String,
    pub exited_at: Option<String>,
    pub exit_code: Option<i64>,
    pub counters: StoredResourceCounters,
    pub dropped_output_bytes: u64,
}

/// One output frame retained for replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredOutputFrame {
    pub seq: u64,
    pub stream: String,
    pub payload: Vec<u8>,
}

/// Durable output replay starting at a caller-supplied monotonic offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredOutputReplay {
    pub frames: Vec<StoredOutputFrame>,
    pub next_offset: u64,
    pub truncated: bool,
}

impl Store {
    /// Opens (creating if absent) the database at `path`, configures it for
    /// durable local operation, verifies integrity, and brings the schema to
    /// [`CURRENT_SCHEMA_VERSION`] through transactional migrations. Reopening
    /// an up-to-date database applies nothing and changes nothing.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PersistenceError> {
        let conn = Connection::open(path.as_ref()).map_err(PersistenceError::Open)?;
        configure(&conn)?;
        run_migrations(&conn, MIGRATIONS)?;
        Ok(Self { conn })
    }

    /// Opens a private in-memory store with the same configuration and
    /// migrations. Useful for tests and for ephemeral diagnostic runs.
    pub fn open_in_memory() -> Result<Self, PersistenceError> {
        let conn = Connection::open_in_memory().map_err(PersistenceError::Open)?;
        configure(&conn)?;
        run_migrations(&conn, MIGRATIONS)?;
        Ok(Self { conn })
    }

    /// The highest migration recorded in the ledger: the on-disk schema
    /// version. Independent of RPC method versions and package semver.
    pub fn schema_version(&self) -> Result<i64, PersistenceError> {
        self.conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM migrations",
                [],
                |row| row.get(0),
            )
            .map_err(|source| PersistenceError::Sqlite {
                context: "reading schema version",
                source,
            })
    }

    /// Reads one durable runtime metadata value.
    pub fn get_meta(&self, key: &str) -> Result<Option<String>, PersistenceError> {
        self.conn
            .query_row(
                "SELECT value FROM runtime_meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| PersistenceError::Sqlite {
                context: "reading runtime metadata",
                source,
            })
    }

    /// Writes one durable runtime metadata value. The upsert is a single
    /// statement, therefore atomic: a crash leaves either the old or the new
    /// value, never a torn one. The caller commits any surrounding durability
    /// boundary before acknowledging externally.
    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), PersistenceError> {
        self.conn
            .execute(
                "INSERT INTO runtime_meta (key, value, updated_at_utc)
                 VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                 ON CONFLICT(key) DO UPDATE SET
                     value = excluded.value,
                     updated_at_utc = excluded.updated_at_utc",
                params![key, value],
            )
            .map_err(|source| PersistenceError::Sqlite {
                context: "writing runtime metadata",
                source,
            })?;
        Ok(())
    }

    /// Inserts the durable `STARTING` record before the OS process is spawned.
    pub fn insert_supervised_process(
        &mut self,
        id: &str,
        program: &str,
        args_json: &str,
        cwd: Option<&str>,
        run_mode: &str,
    ) -> Result<(), PersistenceError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|source| sqlite_error("beginning process insert", source))?;
        tx.execute(
            "INSERT INTO supervised_processes
             (id, status, program, args_json, cwd, run_mode, started_at_utc)
             VALUES (?1, 'STARTING', ?2, ?3, ?4, ?5,
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![id, program, args_json, cwd, run_mode],
        )
        .map_err(|source| sqlite_error("inserting supervised process", source))?;
        tx.commit()
            .map_err(|source| sqlite_error("committing process insert", source))
    }

    /// Records the PID only after the supervisor has attached the process tree.
    pub fn mark_process_running(&mut self, id: &str, pid: u32) -> Result<(), PersistenceError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|source| sqlite_error("beginning process start transition", source))?;
        tx.execute(
            "UPDATE supervised_processes SET status = 'RUNNING', pid = ?2 WHERE id = ?1",
            params![id, i64::from(pid)],
        )
        .map_err(|source| sqlite_error("marking supervised process running", source))?;
        tx.commit()
            .map_err(|source| sqlite_error("committing process start transition", source))
    }

    /// Records a terminal process state and the final resource counters.
    pub fn mark_process_finished(
        &mut self,
        id: &str,
        status: &str,
        exit_code: Option<i32>,
        counters: &StoredResourceCounters,
    ) -> Result<(), PersistenceError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|source| sqlite_error("beginning process finish transition", source))?;
        tx.execute(
            "UPDATE supervised_processes SET
                 status = ?2,
                 exited_at_utc = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 exit_code = ?3,
                 duration_ms = ?4,
                 stdout_bytes = ?5,
                 stderr_bytes = ?6,
                 cpu_ms = ?7,
                 peak_memory_bytes = ?8
             WHERE id = ?1
               AND (?2 != 'EXITED' OR status IN ('STARTING', 'RUNNING'))",
            params![
                id,
                status,
                exit_code.map(i64::from),
                counters.duration_ms.map(sqlite_u64),
                sqlite_u64(counters.stdout_bytes),
                sqlite_u64(counters.stderr_bytes),
                counters.cpu_ms.map(sqlite_u64),
                counters.peak_memory_bytes.map(sqlite_u64),
            ],
        )
        .map_err(|source| sqlite_error("finishing supervised process", source))?;
        tx.commit()
            .map_err(|source| sqlite_error("committing process finish transition", source))
    }

    /// Appends one replay frame and trims the oldest payload bytes in the same
    /// transaction when the per-process durable cap is exceeded.
    pub fn append_output_frame(
        &mut self,
        process_id: &str,
        seq: u64,
        stream: &str,
        payload: &[u8],
    ) -> Result<(), PersistenceError> {
        self.append_output_frame_bounded(process_id, seq, stream, payload, PROCESS_OUTPUT_CAP_BYTES)
    }

    fn append_output_frame_bounded(
        &mut self,
        process_id: &str,
        seq: u64,
        stream: &str,
        payload: &[u8],
        cap_bytes: u64,
    ) -> Result<(), PersistenceError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|source| sqlite_error("beginning output append", source))?;
        let inserted = tx
            .execute(
                "INSERT OR IGNORE INTO process_output_frames
                 (process_id, seq, stream, payload, created_at_utc)
                 VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![process_id, sqlite_u64(seq), stream, payload],
            )
            .map_err(|source| sqlite_error("appending process output", source))?;
        tx.execute(
            "UPDATE supervised_processes
             SET next_output_offset = MAX(next_output_offset, ?2)
             WHERE id = ?1",
            params![process_id, sqlite_u64(seq.saturating_add(1))],
        )
        .map_err(|source| sqlite_error("advancing process output offset", source))?;

        let mut dropped = 0_u64;
        if inserted != 0 {
            let mut total: u64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(length(payload)), 0)
                     FROM process_output_frames WHERE process_id = ?1",
                    [process_id],
                    |row| row.get(0),
                )
                .map_err(|source| sqlite_error("measuring retained process output", source))?;
            while total > cap_bytes {
                let oldest: Option<(i64, u64)> = tx
                    .query_row(
                        "SELECT id, length(payload) FROM process_output_frames
                         WHERE process_id = ?1 ORDER BY id LIMIT 1",
                        [process_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(|source| sqlite_error("finding oldest process output", source))?;
                let Some((id, bytes)) = oldest else {
                    break;
                };
                tx.execute("DELETE FROM process_output_frames WHERE id = ?1", [id])
                    .map_err(|source| sqlite_error("trimming process output", source))?;
                total = total.saturating_sub(bytes);
                dropped = dropped.saturating_add(bytes);
            }
        }
        if dropped != 0 {
            tx.execute(
                "UPDATE supervised_processes
                 SET dropped_output_bytes = dropped_output_bytes + ?2 WHERE id = ?1",
                params![process_id, sqlite_u64(dropped)],
            )
            .map_err(|source| sqlite_error("recording dropped process output", source))?;
        }
        tx.commit()
            .map_err(|source| sqlite_error("committing output append", source))
    }

    /// Preserves the supervisor's cumulative drop counter when its hot spool
    /// fell behind before the database writer could replay it.
    pub fn record_dropped_output_bytes(
        &mut self,
        process_id: &str,
        dropped_bytes: u64,
    ) -> Result<(), PersistenceError> {
        self.conn
            .execute(
                "UPDATE supervised_processes
                 SET dropped_output_bytes = MAX(dropped_output_bytes, ?2) WHERE id = ?1",
                params![process_id, sqlite_u64(dropped_bytes)],
            )
            .map_err(|source| sqlite_error("recording supervisor output loss", source))?;
        Ok(())
    }

    /// Reads the retained output tail at or after `offset`.
    pub fn process_output(
        &self,
        process_id: &str,
        offset: u64,
    ) -> Result<Option<StoredOutputReplay>, PersistenceError> {
        let state: Option<(u64, u64)> = self
            .conn
            .query_row(
                "SELECT next_output_offset, dropped_output_bytes
                 FROM supervised_processes WHERE id = ?1",
                [process_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|source| sqlite_error("reading process output state", source))?;
        let Some((next_offset, dropped_bytes)) = state else {
            return Ok(None);
        };
        let oldest: Option<u64> = self
            .conn
            .query_row(
                "SELECT MIN(seq) FROM process_output_frames WHERE process_id = ?1",
                [process_id],
                |row| row.get(0),
            )
            .map_err(|source| sqlite_error("reading oldest process output offset", source))?;
        let mut stmt = self
            .conn
            .prepare(
                "SELECT seq, stream, payload FROM process_output_frames
                 WHERE process_id = ?1 AND seq >= ?2 ORDER BY seq",
            )
            .map_err(|source| sqlite_error("preparing process output replay", source))?;
        let rows = stmt
            .query_map(params![process_id, sqlite_u64(offset)], |row| {
                Ok(StoredOutputFrame {
                    seq: row.get(0)?,
                    stream: row.get(1)?,
                    payload: row.get(2)?,
                })
            })
            .map_err(|source| sqlite_error("reading process output replay", source))?;
        let frames = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| sqlite_error("decoding process output replay", source))?;
        let oldest_retained = oldest.unwrap_or(next_offset);
        Ok(Some(StoredOutputReplay {
            frames,
            next_offset,
            truncated: dropped_bytes != 0 && offset < oldest_retained,
        }))
    }

    /// Lists durable process state in startup order.
    pub fn list_supervised_processes(&self) -> Result<Vec<StoredProcess>, PersistenceError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, status, started_at_utc, exited_at_utc, exit_code,
                        duration_ms, stdout_bytes, stderr_bytes, cpu_ms,
                        peak_memory_bytes, dropped_output_bytes
                 FROM supervised_processes ORDER BY started_at_utc, id",
            )
            .map_err(|source| sqlite_error("preparing supervised process list", source))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(StoredProcess {
                    id: row.get(0)?,
                    status: row.get(1)?,
                    started_at: row.get(2)?,
                    exited_at: row.get(3)?,
                    exit_code: row.get(4)?,
                    counters: StoredResourceCounters {
                        duration_ms: row.get(5)?,
                        stdout_bytes: row.get(6)?,
                        stderr_bytes: row.get(7)?,
                        cpu_ms: row.get(8)?,
                        peak_memory_bytes: row.get(9)?,
                    },
                    dropped_output_bytes: row.get(10)?,
                })
            })
            .map_err(|source| sqlite_error("listing supervised processes", source))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|source| sqlite_error("decoding supervised process list", source))
    }

    /// Converts every process that could have been live at the prior Host's
    /// death into an explicit interruption plus its durable audit record.
    pub fn interrupt_active_processes(&mut self, reason: &str) -> Result<usize, PersistenceError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|source| sqlite_error("beginning process interruption recovery", source))?;
        tx.execute(
            "INSERT INTO process_interruptions (process_id, detected_at_utc, reason)
             SELECT id, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?1
             FROM supervised_processes WHERE status IN ('STARTING', 'RUNNING')",
            [reason],
        )
        .map_err(|source| sqlite_error("recording process interruptions", source))?;
        let interrupted = tx
            .execute(
                "UPDATE supervised_processes SET
                     status = 'INTERRUPTED',
                     exited_at_utc = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE status IN ('STARTING', 'RUNNING')",
                [],
            )
            .map_err(|source| sqlite_error("marking interrupted processes", source))?;
        tx.commit()
            .map_err(|source| sqlite_error("committing process interruption recovery", source))?;
        Ok(interrupted)
    }

    /// Finalizes any rows still active after the supervisor has completed a
    /// graceful all-process shutdown.
    pub fn stop_active_processes(&mut self) -> Result<usize, PersistenceError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|source| sqlite_error("beginning graceful process shutdown", source))?;
        let stopped = tx
            .execute(
                "UPDATE supervised_processes SET
                     status = 'STOPPED',
                     exited_at_utc = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE status IN ('STARTING', 'RUNNING')",
                [],
            )
            .map_err(|source| sqlite_error("stopping active process records", source))?;
        tx.commit()
            .map_err(|source| sqlite_error("committing graceful process shutdown", source))?;
        Ok(stopped)
    }
}

fn sqlite_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn sqlite_error(context: &'static str, source: rusqlite::Error) -> PersistenceError {
    PersistenceError::Sqlite { context, source }
}

/// Applies the durability-relevant pragmas and verifies the file is readable.
/// WAL keeps finalized writes durable while readers stream; FULL sync keeps
/// acknowledged commits resilient to power loss; the busy timeout absorbs
/// transient contention from a second local process.
fn configure(conn: &Connection) -> Result<(), PersistenceError> {
    conn.busy_timeout(std::time::Duration::from_millis(BUSY_TIMEOUT_MS))
        .map_err(|source| PersistenceError::Sqlite {
            context: "setting busy timeout",
            source,
        })?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|source| PersistenceError::Sqlite {
            context: "enabling WAL journal mode",
            source,
        })?;
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(|source| PersistenceError::Sqlite {
            context: "setting synchronous mode",
            source,
        })?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|source| PersistenceError::Sqlite {
            context: "enabling foreign keys",
            source,
        })?;
    // quick_check is a lighter integrity scan than integrity_check and runs
    // before anything trusts the file. In-memory databases always report ok.
    let report: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|source| PersistenceError::Sqlite {
            context: "running integrity check",
            source,
        })?;
    if report != "ok" {
        return Err(PersistenceError::Corrupt(report));
    }
    Ok(())
}

/// Creates the migration ledger outside the tracked chain, then applies every
/// listed migration that the database has not yet recorded, oldest first.
///
/// Each application is exactly one transaction: the step's SQL plus the ledger
/// row commit together or not at all, so an interrupted Host restarts into the
/// previous known-good schema and simply retries the step.
pub(crate) fn run_migrations(
    conn: &Connection,
    migrations: &[Migration],
) -> Result<(), PersistenceError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at_utc TEXT NOT NULL
        ) STRICT",
    )
    .map_err(|source| PersistenceError::Sqlite {
        context: "creating migrations ledger",
        source,
    })?;

    let mut applied: Vec<i64> = Vec::new();
    {
        let mut stmt = conn
            .prepare("SELECT version FROM migrations")
            .map_err(|source| PersistenceError::Sqlite {
                context: "reading applied migrations",
                source,
            })?;
        let rows = stmt
            .query_map([], |row| row.get::<_, i64>(0))
            .map_err(|source| PersistenceError::Sqlite {
                context: "reading applied migrations",
                source,
            })?;
        for row in rows {
            applied.push(row.map_err(|source| PersistenceError::Sqlite {
                context: "reading applied migrations",
                source,
            })?);
        }
    }

    // Refuse databases from newer Host builds before touching anything.
    let on_disk = applied.iter().copied().max().unwrap_or(0);
    let known = migrations.iter().map(|m| m.version).max().unwrap_or(0);
    if on_disk > known {
        return Err(PersistenceError::DatabaseTooNew {
            on_disk,
            supported: CURRENT_SCHEMA_VERSION.max(known),
        });
    }

    for migration in migrations {
        if applied.contains(&migration.version) {
            continue;
        }
        apply_migration(conn, migration)?;
    }
    Ok(())
}

/// Runs one migration's statements plus its ledger row inside a single
/// immediate transaction, so concurrent local readers see either the whole
/// step or none of it. Uses explicit `BEGIN IMMEDIATE`/`COMMIT`/`ROLLBACK`
/// rather than a borrowed `Transaction`, so it works from a shared
/// connection reference and stays stable across rusqlite versions.
fn apply_migration(conn: &Connection, migration: &Migration) -> Result<(), PersistenceError> {
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|source| PersistenceError::Sqlite {
            context: "beginning migration transaction",
            source,
        })?;
    let outcome = (|| -> Result<(), PersistenceError> {
        for statement in migration.statements {
            conn.execute_batch(statement).map_err(|source| {
                // Nothing committed yet: rolling back undoes every statement
                // so far, and the ledger row was never inserted, so a retry
                // re-runs the whole step on a pristine schema.
                PersistenceError::MigrationFailed {
                    version: migration.version,
                    source,
                }
            })?;
        }
        conn.execute(
            "INSERT INTO migrations (version, name, applied_at_utc)
             VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![migration.version, migration.name],
        )
        .map_err(|source| PersistenceError::Sqlite {
            context: "recording migration in ledger",
            source,
        })?;
        Ok(())
    })();
    match outcome {
        Ok(()) => conn
            .execute_batch("COMMIT")
            .map_err(|source| PersistenceError::Sqlite {
                context: "committing migration transaction",
                source,
            }),
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// Unique per-test database paths without a dev-dependency on tempfile.
    fn temp_db_path(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lazarus-hostd-persistence-{tag}-{}-{n}.db",
            std::process::id(),
        ));
        path
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn fresh_store_applies_all_migrations_once() {
        let store = Store::open_in_memory().expect("fresh in-memory store");
        assert_eq!(
            store.schema_version().expect("schema version"),
            CURRENT_SCHEMA_VERSION
        );
        let applied: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM migrations", [], |row| row.get(0))
            .expect("count migrations");
        assert_eq!(applied, MIGRATIONS.len() as i64);
        // Version 1's table exists and starts empty.
        assert_eq!(store.get_meta("anything").expect("read meta"), None);
        for table in [
            "supervised_processes",
            "process_output_frames",
            "process_interruptions",
        ] {
            let exists: i64 = store
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .expect("table probe");
            assert_eq!(exists, 1, "migration 2 must create {table}");
        }
    }

    #[test]
    fn reopening_a_v1_database_applies_v2_without_losing_rows() {
        let path = temp_db_path("upgrade-v1");
        {
            let conn = Connection::open(&path).expect("create v1 database");
            configure(&conn).expect("configure v1 database");
            run_migrations(&conn, &MIGRATIONS[..1]).expect("apply migration 1");
            conn.execute(
                "INSERT INTO runtime_meta (key, value, updated_at_utc)
                 VALUES ('marker', 'from-v1', '2026-08-24T00:00:00Z')",
                [],
            )
            .expect("seed v1 row");
        }

        let store = Store::open(&path).expect("upgrade v1 database");
        assert_eq!(
            store
                .get_meta("marker")
                .expect("read preserved row")
                .as_deref(),
            Some("from-v1")
        );
        assert_eq!(store.schema_version().expect("schema version"), 2);
        let table_exists: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'supervised_processes'",
                [],
                |row| row.get(0),
            )
            .expect("supervised process table probe");
        assert_eq!(table_exists, 1);
        cleanup(&path);
    }

    #[test]
    fn supervised_process_rows_round_trip() {
        let store = Store::open_in_memory().expect("store");
        store
            .conn
            .execute(
                "INSERT INTO supervised_processes
                 (id, status, program, args_json, cwd, run_mode, pid, started_at_utc)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    "0198e550-c9be-7000-8000-000000000001",
                    "RUNNING",
                    "git",
                    r#"["status","--short"]"#,
                    "D:/project/lazarus",
                    "PIPED",
                    4242,
                    "2026-08-24T12:00:00Z",
                ],
            )
            .expect("insert supervised process");

        let row: (String, String, String, Option<i64>) = store
            .conn
            .query_row(
                "SELECT status, program, args_json, pid FROM supervised_processes WHERE id = ?1",
                ["0198e550-c9be-7000-8000-000000000001"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("query supervised process");
        assert_eq!(
            row,
            (
                "RUNNING".to_string(),
                "git".to_string(),
                r#"["status","--short"]"#.to_string(),
                Some(4242),
            )
        );
    }

    #[test]
    fn process_accessors_transition_and_list_rows() {
        let mut store = Store::open_in_memory().expect("store");
        let id = "0198e550-c9be-7000-8000-000000000002";
        store
            .insert_supervised_process(id, "git", r#"["status"]"#, None, "PIPED")
            .expect("insert starting row");
        store.mark_process_running(id, 4242).expect("mark running");
        store
            .mark_process_finished(
                id,
                "EXITED",
                Some(0),
                &StoredResourceCounters {
                    duration_ms: Some(25),
                    stdout_bytes: 7,
                    stderr_bytes: 3,
                    cpu_ms: Some(2),
                    peak_memory_bytes: Some(1024),
                },
            )
            .expect("mark exited");

        let rows = store.list_supervised_processes().expect("list processes");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].status, "EXITED");
        assert_eq!(rows[0].exit_code, Some(0));
        assert_eq!(rows[0].counters.stdout_bytes, 7);
        assert!(rows[0].exited_at.is_some());
    }

    #[test]
    fn durable_output_replay_trims_oldest_bytes_and_reports_the_gap() {
        let mut store = Store::open_in_memory().expect("store");
        let id = "0198e550-c9be-7000-8000-000000000003";
        store
            .insert_supervised_process(id, "echo", "[]", None, "PIPED")
            .expect("insert process");
        store
            .append_output_frame_bounded(id, 0, "STDOUT", b"abc", 5)
            .expect("append first frame");
        store
            .append_output_frame_bounded(id, 1, "STDERR", b"def", 5)
            .expect("append and trim");

        let replay = store
            .process_output(id, 0)
            .expect("read replay")
            .expect("known process");
        assert!(replay.truncated);
        assert_eq!(replay.next_offset, 2);
        assert_eq!(replay.frames.len(), 1);
        assert_eq!(replay.frames[0].seq, 1);
        assert_eq!(replay.frames[0].payload, b"def");
        assert_eq!(
            store.list_supervised_processes().unwrap()[0].dropped_output_bytes,
            3
        );
    }

    #[test]
    fn startup_recovery_records_each_interrupted_process_once() {
        let mut store = Store::open_in_memory().expect("store");
        for id in [
            "0198e550-c9be-7000-8000-000000000004",
            "0198e550-c9be-7000-8000-000000000005",
        ] {
            store
                .insert_supervised_process(id, "sleep", "[]", None, "PIPED")
                .expect("insert process");
        }
        store
            .mark_process_running("0198e550-c9be-7000-8000-000000000005", 7)
            .expect("mark running");

        assert_eq!(store.interrupt_active_processes("host died").unwrap(), 2);
        assert_eq!(store.interrupt_active_processes("host died").unwrap(), 0);
        assert!(
            store
                .list_supervised_processes()
                .unwrap()
                .iter()
                .all(|process| process.status == "INTERRUPTED")
        );
        let interruptions: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM process_interruptions", [], |row| {
                row.get(0)
            })
            .expect("count interruption records");
        assert_eq!(interruptions, 2);
    }

    #[test]
    fn reopening_applies_nothing_and_reports_the_same_version() {
        let path = temp_db_path("idempotent");
        let first_version = {
            let store = Store::open(&path).expect("first open");
            store.set_meta("marker", "v1").expect("write marker");
            store.schema_version().expect("schema version")
        };
        let second_version = {
            let store = Store::open(&path).expect("reopen");
            assert_eq!(
                store.get_meta("marker").expect("read marker").as_deref(),
                Some("v1"),
                "reopen must not disturb existing data"
            );
            store.schema_version().expect("schema version")
        };
        cleanup(&path);
        assert_eq!(first_version, second_version);
    }

    #[test]
    fn metadata_survives_a_full_restart_cycle() {
        let path = temp_db_path("restart");
        {
            let store = Store::open(&path).expect("first boot");
            store
                .set_meta("startup_epoch", "1724400000000")
                .expect("set");
            store.set_meta("outage_id", "abc-123").expect("set");
        }
        // Simulated process death: the handle is gone, only the file remains.
        {
            let store = Store::open(&path).expect("second boot recovers cleanly");
            assert_eq!(
                store.get_meta("startup_epoch").expect("read").as_deref(),
                Some("1724400000000")
            );
            assert_eq!(
                store.get_meta("outage_id").expect("read").as_deref(),
                Some("abc-123")
            );
            store
                .set_meta("startup_epoch", "1724400009999")
                .expect("overwrite");
        }
        let store = Store::open(&path).expect("third boot");
        assert_eq!(
            store.get_meta("startup_epoch").expect("read").as_deref(),
            Some("1724400009999"),
            "the newest value wins after another restart"
        );
        cleanup(&path);
    }

    /// The test-only seam for failure injection: a migration whose second
    /// statement fails after its first succeeded must roll back completely -
    /// no partial objects, no ledger row - and leave the database usable so
    /// startup recovery can retry with a fixed chain.
    #[test]
    fn failed_migration_rolls_back_and_leaves_the_database_recoverable() {
        let path = temp_db_path("rollback");
        {
            let store = Store::open(&path).expect("baseline store at current schema");
            assert_eq!(
                store.schema_version().expect("version"),
                CURRENT_SCHEMA_VERSION
            );
        }
        let conn = Connection::open(&path).expect("raw reopen");
        configure(&conn).expect("configure raw connection");

        let doomed = Migration {
            version: 3,
            name: "doomed_step",
            statements: &[
                "CREATE TABLE partial_artifact (id INTEGER)",
                // Fails midway: the statement above must roll back with it.
                "INSERT INTO no_such_table VALUES (1)",
            ],
        };
        let mut doomed_chain = MIGRATIONS.to_vec();
        doomed_chain.push(doomed);
        let error = run_migrations(&conn, &doomed_chain).expect_err("the doomed migration fails");
        assert!(
            matches!(error, PersistenceError::MigrationFailed { version: 3, .. }),
            "expected a typed migration failure, got {error}"
        );

        // Nothing leaked: no ledger row for v3 and no half-created table...
        let leaked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = 3",
                [],
                |row| row.get(0),
            )
            .expect("ledger query");
        assert_eq!(leaked, 0, "a failed migration must not be recorded");
        let partial: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'partial_artifact'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master probe");
        assert_eq!(partial, 0, "the failed step's DDL must roll back");

        // ...and the database still works: a corrected chain applies cleanly.
        let repaired = Migration {
            version: 3,
            name: "repaired_step",
            statements: &["CREATE TABLE recovered (id INTEGER)"],
        };
        let mut repaired_chain = MIGRATIONS.to_vec();
        repaired_chain.push(repaired);
        run_migrations(&conn, &repaired_chain).expect("recovery succeeds");
        drop(conn);

        // The repaired file is now at schema v3, which this binary's shipped
        // chain (v2 max) does not know: Store::open must refuse it rather
        // than misread a newer schema.
        let error = Store::open(&path).expect_err("must refuse the newer schema");
        assert!(
            matches!(error, PersistenceError::DatabaseTooNew { on_disk: 3, .. }),
            "expected DatabaseTooNew, got {error}"
        );
        cleanup(&path);
    }

    #[test]
    fn wal_and_foreign_key_pragmas_are_active_on_file_backed_stores() {
        let path = temp_db_path("pragmas");
        let store = Store::open(&path).expect("file-backed store");
        let journal: String = store
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal mode");
        assert_eq!(journal.to_ascii_lowercase(), "wal");
        let foreign_keys: i64 = store
            .conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign keys pragma");
        assert_eq!(foreign_keys, 1);
        cleanup(&path);
    }

    #[test]
    fn a_database_from_a_newer_host_is_refused() {
        let path = temp_db_path("too-new");
        let conn = Connection::open(&path).expect("create file");
        configure(&conn).expect("configure");
        conn.execute_batch("CREATE TABLE migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at_utc TEXT NOT NULL) STRICT;
            INSERT INTO migrations VALUES (99, 'future', '2026-08-24T00:00:00Z')")
            .expect("seed future schema");
        drop(conn);
        let error = Store::open(&path).expect_err("must refuse a newer database");
        assert!(
            matches!(error, PersistenceError::DatabaseTooNew { on_disk: 99, .. }),
            "expected DatabaseTooNew, got {error}"
        );
        cleanup(&path);
    }
}
