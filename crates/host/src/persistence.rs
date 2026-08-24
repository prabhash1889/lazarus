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

use rusqlite::{Connection, OptionalExtension, params};

/// Highest migration version this binary knows how to reach. A database from
/// a newer Host must refuse to open rather than be silently misread.
pub const CURRENT_SCHEMA_VERSION: i64 = 1;

/// How long a writer waits for a competing local writer before giving up.
const BUSY_TIMEOUT_MS: u64 = 5_000;

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
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "bootstrap_core_tables",
    statements: &["CREATE TABLE IF NOT EXISTS runtime_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at_utc TEXT NOT NULL
        ) STRICT"],
}];

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
            let store = Store::open(&path).expect("baseline store at v1");
            assert_eq!(store.schema_version().expect("version"), 1);
        }
        let conn = Connection::open(&path).expect("raw reopen");
        configure(&conn).expect("configure raw connection");

        let doomed = Migration {
            version: 2,
            name: "doomed_step",
            statements: &[
                "CREATE TABLE partial_artifact (id INTEGER)",
                // Fails midway: the statement above must roll back with it.
                "INSERT INTO no_such_table VALUES (1)",
            ],
        };
        let error = run_migrations(&conn, &[MIGRATIONS[0], doomed])
            .expect_err("the doomed migration fails");
        assert!(
            matches!(error, PersistenceError::MigrationFailed { version: 2, .. }),
            "expected a typed migration failure, got {error}"
        );

        // Nothing leaked: no ledger row for v2 and no half-created table...
        let leaked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = 2",
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
            version: 2,
            name: "repaired_step",
            statements: &["CREATE TABLE recovered (id INTEGER)"],
        };
        run_migrations(&conn, &[MIGRATIONS[0], repaired]).expect("recovery succeeds");
        drop(conn);

        // The repaired file is now at schema v2, which this binary's shipped
        // chain (v1 max) does not know: Store::open must refuse it rather
        // than misread a newer schema.
        let error = Store::open(&path).expect_err("must refuse the newer schema");
        assert!(
            matches!(error, PersistenceError::DatabaseTooNew { on_disk: 2, .. }),
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
