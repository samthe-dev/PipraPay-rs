//! SQLite access: WAL mode, one writer, small read pool.
//!
//! Money is stored as `INTEGER` minor units (amount x 10^8, matching PHP
//! `decimal(20,8)`), never as float. Dates are TEXT `Y-m-d H:i:s` UTC,
//! matching the PHP `varchar(20)` convention. The `'--'` sentinel is kept
//! verbatim in the database for PHP compatibility and normalised to
//! `None` only at the Rust boundary.
//!
//! `balance` in `pp_sms_data` stays TEXT like PHP (`varchar(70)`), holding
//! the money string as the SMS reported it; `amount` is the INTEGER minor
//! units. `pp_balance_verification.current_balance` is INTEGER minor units.

use rusqlite::{Connection, Result};
use std::sync::Mutex;

pub struct Db {
    conn: Mutex<Connection>,
}

const MIGRATION_001: &str = include_str!("../migrations/001_core.sql");

/// Tables created by 001 (all 23), each needing created_date/updated_date.
const ALL_TABLES: &[&str] = &[
    "pp_addon",
    "pp_addon_parameter",
    "pp_admin",
    "pp_api",
    "pp_balance_verification",
    "pp_brands",
    "pp_browser_log",
    "pp_currency",
    "pp_customer",
    "pp_device",
    "pp_domain",
    "pp_env",
    "pp_faq",
    "pp_gateways",
    "pp_gateways_parameter",
    "pp_invoice",
    "pp_invoice_items",
    "pp_payment_link",
    "pp_payment_link_field",
    "pp_permission",
    "pp_sms_data",
    "pp_transaction",
    "pp_webhook_log",
];

#[derive(Debug, Clone)]
pub struct DeviceRow {
    pub id: i64,
    pub d_id: String,
    pub device_id: String,
    pub otp: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct BalanceRow {
    pub id: i64,
    pub current_balance: i64,
    pub simslot: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct SmsRow {
    pub id: i64,
    pub amount: i64,
    pub balance: String,
}

#[derive(Debug, Clone)]
pub struct SmsListRow {
    pub id: i64,
    pub sender: String,
    pub message: String,
    pub reason: String,
    pub simslot: String,
    pub created_date: String,
    pub status: String,
}

fn ensure_column(conn: &Connection, table: &str, column: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(());
        }
    }
    conn.execute_batch(&format!(
        "ALTER TABLE {table} ADD COLUMN {column} TEXT NOT NULL DEFAULT '--';"
    ))
}

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
        // 002: created_date / updated_date on every table, idempotent.
        for table in ALL_TABLES {
            ensure_column(&conn, table, "created_date")?;
            ensure_column(&conn, table, "updated_date")?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Look up a device by its OTP token, any status.
    /// PHP login does `WHERE otp = :otp` with no status filter.
    pub fn device_by_otp(&self, otp: &str) -> Result<Option<DeviceRow>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT id, d_id, device_id, otp, status FROM pp_device WHERE otp = ?1",
        )?;
        let mut rows = stmt.query([otp])?;
        if let Some(row) = rows.next()? {
            Ok(Some(DeviceRow {
                id: row.get(0)?,
                d_id: row.get(1)?,
                device_id: row.get(2)?,
                otp: row.get(3)?,
                status: row.get(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Look up a device by its current OTP token with status `used`.
    /// Used by every companion call after login.
    pub fn device_by_token(&self, token: &str) -> Result<Option<DeviceRow>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT id, d_id, device_id, otp, status FROM pp_device
             WHERE otp = ?1 AND status = 'used'",
        )?;
        let mut rows = stmt.query([token])?;
        if let Some(row) = rows.next()? {
            Ok(Some(DeviceRow {
                id: row.get(0)?,
                d_id: row.get(1)?,
                device_id: row.get(2)?,
                otp: row.get(3)?,
                status: row.get(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// PHP login: rotate the OTP to a fresh 10-digit ID, fill in the
    /// device details, set status `used`.
    #[allow(clippy::too_many_arguments)]
    pub fn login_device(
        &self,
        id: i64,
        new_otp: &str,
        name: &str,
        model: &str,
        android_level: &str,
        app_version: &str,
        now: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("db lock");
        conn.execute(
            "UPDATE pp_device SET otp = ?1, name = ?2, model = ?3,
             android_level = ?4, app_version = ?5, status = 'used',
             updated_date = ?6 WHERE id = ?7",
            rusqlite::params![
                new_otp,
                name,
                model,
                android_level,
                app_version,
                now,
                id
            ],
        )?;
        Ok(())
    }

    pub fn touch_device_last_sync(&self, id: i64, now: &str) -> Result<()> {
        let conn = self.conn.lock().expect("db lock");
        conn.execute(
            "UPDATE pp_device SET last_sync = ?1, updated_date = ?1 WHERE id = ?2",
            rusqlite::params![now, id],
        )?;
        Ok(())
    }

    /// True when a row already exists for this (sender_key, trx_id).
    /// PHP checks any row, regardless of status.
    pub fn sms_exists(&self, sender_key: &str, trx_id: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT 1 FROM pp_sms_data WHERE sender_key = ?1 AND trx_id = ?2 LIMIT 1",
        )?;
        let mut rows = stmt.query(rusqlite::params![sender_key, trx_id])?;
        Ok(rows.next()?.is_some())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_sms_data(
        &self,
        source: &str,
        device_id: &str,
        sender: &str,
        sender_key: &str,
        simslot: &str,
        number: &str,
        amount: i64,
        currency: &str,
        trx_id: &str,
        balance: &str,
        type_: &str,
        status: &str,
        message: &str,
        reason: &str,
        now: &str,
    ) -> Result<i64> {
        let conn = self.conn.lock().expect("db lock");
        conn.execute(
            "INSERT INTO pp_sms_data
             (source, device_id, sender, sender_key, simslot, number, amount,
              currency, trx_id, balance, type, status, message, reason,
              created_date, updated_date)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
            rusqlite::params![
                source, device_id, sender, sender_key, simslot, number, amount, currency,
                trx_id, balance, type_, status, message, reason, now
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Minimal error row: PHP inserts only source/device/sender/simslot/
    /// status/message/reason/dates for unparsable and duplicate SMS.
    pub fn insert_sms_error(
        &self,
        device_id: &str,
        sender: &str,
        simslot: &str,
        status: &str,
        message: &str,
        reason: &str,
        now: &str,
    ) -> Result<i64> {
        let conn = self.conn.lock().expect("db lock");
        conn.execute(
            "INSERT INTO pp_sms_data
             (source, device_id, sender, simslot, status, message, reason,
              created_date, updated_date)
             VALUES ('app', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            rusqlite::params![device_id, sender, simslot, status, message, reason, now],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Admin session cookie -> admin id (PHP `account-information`
    /// resolves the device's `d_id` through `pp_browser_log`).
    pub fn browser_log_a_id(&self, cookie: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt =
            conn.prepare("SELECT a_id FROM pp_browser_log WHERE cookie = ?1")?;
        let mut rows = stmt.query([cookie])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn admin_by_a_id(&self, a_id: &str) -> Result<Option<(String, String)>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT full_name, email FROM pp_admin WHERE a_id = ?1",
        )?;
        let mut rows = stmt.query([a_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?)))
        } else {
            Ok(None)
        }
    }

    /// All app-source SMS for a device, newest first, for
    /// `account-information`.
    pub fn sms_for_device(&self, device_id: &str) -> Result<Vec<SmsListRow>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT id, sender, message, reason, simslot, created_date, status
             FROM pp_sms_data WHERE source = 'app' AND device_id = ?1
             ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([device_id], |row| {
            Ok(SmsListRow {
                id: row.get(0)?,
                sender: row.get(1)?,
                message: row.get(2)?,
                reason: row.get(3)?,
                simslot: row.get(4)?,
                created_date: row.get(5)?,
                status: row.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn balance_verification(
        &self,
        device_id: &str,
        sender_key: &str,
        type_: &str,
    ) -> Result<Option<BalanceRow>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT id, current_balance, simslot, status FROM pp_balance_verification
             WHERE device_id = ?1 AND sender_key = ?2 AND type = ?3",
        )?;
        let mut rows = stmt.query(rusqlite::params![device_id, sender_key, type_])?;
        if let Some(row) = rows.next()? {
            Ok(Some(BalanceRow {
                id: row.get(0)?,
                current_balance: row.get(1)?,
                simslot: row.get(2)?,
                status: row.get(3)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn update_balance_verification(
        &self,
        id: i64,
        current_balance: i64,
        now: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("db lock");
        conn.execute(
            "UPDATE pp_balance_verification SET current_balance = ?1,
             updated_date = ?2 WHERE id = ?3",
            rusqlite::params![current_balance, now, id],
        )?;
        Ok(())
    }

    /// Port of PHP `reconcileByLongestChain()`: starting from the stored
    /// canonical balance, follow the longest chain of SMS where each
    /// next balance == prev balance + amount, approve that chain and set
    /// the verification balance to the chain's last balance.
    pub fn reconcile_by_longest_chain(
        &self,
        device_id: &str,
        sender_key: &str,
        type_: &str,
        now: &str,
    ) -> Result<()> {
        let Some(bv) = self.balance_verification(device_id, sender_key, type_)? else {
            return Ok(());
        };
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(
            "SELECT id, amount, balance FROM pp_sms_data
             WHERE device_id = ?1 AND sender_key = ?2 AND type = ?3
             AND status IN ('approved', 'awaiting-review') AND source = 'app'
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![device_id, sender_key, type_],
            |row| {
                Ok(SmsRow {
                    id: row.get(0)?,
                    amount: row.get(1)?,
                    balance: row.get(2)?,
                })
            },
        )?;
        let mut sms: Vec<SmsRow> = Vec::new();
        for r in rows {
            sms.push(r?);
        }
        drop(stmt);
        if sms.is_empty() {
            return Ok(());
        }

        // prev = balance - amount, bal = balance (minor units).
        let mut next: std::collections::HashMap<i64, Vec<SmsRow>> =
            std::collections::HashMap::new();
        for s in &sms {
            let bal = crate::sms::parse_money(&s.balance);
            if s.amount <= 0 || bal <= 0 {
                continue;
            }
            next.entry(bal - s.amount).or_default().push(s.clone());
        }

        // BFS from the canonical balance, keeping the longest chain found.
        let mut best: Vec<SmsRow> = Vec::new();
        let mut queue: std::collections::VecDeque<i64> =
            std::collections::VecDeque::from([bv.current_balance]);
        while let Some(current) = queue.pop_front() {
            let Some(candidates) = next.get(&current) else {
                continue;
            };
            for sms_item in candidates {
                let mut chain: Vec<SmsRow> = Vec::new();
                let mut temp_current = current;
                let mut temp_next = next.clone();
                while let Some(bucket) = temp_next.get_mut(&temp_current) {
                    if bucket.is_empty() {
                        break;
                    }
                    let item = bucket.remove(0);
                    chain.push(item.clone());
                    temp_current = crate::sms::parse_money(&item.balance);
                }
                if chain.len() > best.len() {
                    best = chain;
                }
            }
        }
        if best.is_empty() {
            return Ok(());
        }

        let ids: Vec<String> = best.iter().map(|s| s.id.to_string()).collect();
        conn.execute(
            &format!(
                "UPDATE pp_sms_data SET status = 'approved', reason = '--',
                 updated_date = ?1 WHERE id IN ({})",
                ids.join(",")
            ),
            rusqlite::params![now],
        )?;
        let final_balance = crate::sms::parse_money(&best.last().unwrap().balance);
        conn.execute(
            "UPDATE pp_balance_verification SET current_balance = ?1,
             updated_date = ?2 WHERE id = ?3",
            rusqlite::params![final_balance, now, bv.id],
        )?;
        Ok(())
    }
}
