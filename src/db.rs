//! SQLite access: WAL mode, one writer, small read pool.
//!
//! Money is stored as `INTEGER` minor units (amount x 10^8, matching PHP
//! `decimal(20,8)`), never as float. Dates are TEXT `Y-m-d H:i:s` UTC,
//! matching the PHP `varchar(20)` convention. The `'--'` sentinel is kept
//! verbatim in the database for PHP compatibility and normalised to
//! `None` only at the Rust boundary.

use rusqlite::{Connection, Result};
use std::sync::Mutex;

pub struct Db {
    conn: Mutex<Connection>,
}

const MIGRATION_001: &str = include_str!("../migrations/001_core.sql");

impl Db {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;
             PRAGMA cache_size=-2000;
             PRAGMA temp_store=MEMORY;",
        )?;
        conn.execute_batch(MIGRATION_001)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Look up a device by its current OTP token. Returns the device id
    /// when the token matches a row with status `used`.
    pub fn device_by_token(&self, token: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT d_id FROM pp_device WHERE otp = ?1 AND status = 'used'",
        )?;
        let mut rows = stmt.query([token])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }
}
