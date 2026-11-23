//! Cron settlement + webhook delivery, mirroring PHP `index.php`
//! `case $path_cron` (secret in the URL, `GET /cron/<secret>`).
//!
//! What PHP's cron does, and what Rust does here:
//! - Verify `pending` transactions against approved SMS (sender_key +
//!   type + trx_id), within the brand's payment tolerance: mark the SMS
//!   `used`, the transaction `completed`, queue a webhook when the
//!   transaction has a webhook URL. (Deliberate fix: PHP's webhook
//!   payload carries the *old* status `pending`; Rust sends `completed`.)
//! - Re-run `reconcileByLongestChain` for every active balance
//!   verification row.
//! - Deliver pending webhook rows (up to 15 per run, attempts limit from
//!   env `geneal-application-settings-webhook_attempts_limit`, default 1):
//!   POST the JSON payload, 200 = completed, otherwise pending, and
//!   canceled once attempts reach the limit.
//! - A run lock (10 minutes, like PHP's cron.lock) prevents overlapping
//!   runs; `last-cron-invocation` is recorded in pp_env.
//!
//! NOT in Phase 1 (documented in STUDY.md): the automatic system
//! update check and the currency auto-update fetch.
//!
//! The endpoint answers immediately and runs the work in the
//! background, like PHP (it echoes the JSON before doing the work).
//! Set `CRON_INTERVAL_SECS` to also run the same work on an internal
//! timer; otherwise an external cron hitting `/cron/<secret>` is the
//! scheduler, exactly as in PHP.

use crate::db::Db;
use crate::merchant::transaction_view;
use crate::now_utc;
use crate::sms::parse_money;
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default, Debug)]
pub struct CronReport {
    pub settled: usize,
    pub webhooks_queued: usize,
    pub reconciled: usize,
    pub webhooks_sent: usize,
    pub webhooks_completed: usize,
}

/// PHP `money_round()`: 2 decimal places.
fn round2(minor: i64) -> i64 {
    ((minor + 500_000) / 1_000_000) * 1_000_000
}

/// PHP `verifyPaymentTolerance()`: paid must be in
/// [checkout, checkout + tolerance].
fn within_tolerance(checkout_minor: i64, paid_minor: i64, tolerance: &str) -> bool {
    let checkout = round2(checkout_minor);
    let paid = round2(paid_minor);
    let tol = round2(parse_money(tolerance));
    if checkout <= 0 || paid <= 0 {
        return false;
    }
    paid >= checkout && paid <= checkout + tol
}

pub async fn run_cron(db: &Db) -> CronReport {
    let mut report = CronReport::default();
    let now = now_utc();

    // 1. Verify pending transactions against approved SMS.
    let pending = db.pending_transactions().unwrap_or_default();
    for tx in &pending {
        let Some(sms) = db
            .approved_sms_by_key(&tx.sender_key, &tx.sender_type, &tx.trx_id)
            .ok()
            .flatten()
        else {
            continue;
        };
        let Some((tolerance, _tz)) = db
            .brand_payment_tolerance(&tx.brand_id)
            .ok()
            .flatten()
        else {
            continue;
        };
        if !within_tolerance(tx.local_net_amount, sms.amount, &tolerance) {
            continue;
        }
        let _ = db.sms_set_status(sms.id, "used", &now);
        let _ = db.complete_transaction(tx.id, &sms.number, &now);
        report.settled += 1;
        let mut settled = tx.clone();
        settled.sender = sms.number.clone();
        settled.status = "completed".to_string();
        let payload = transaction_view(db, &settled, "completed");
        if !tx.webhook_url.is_empty() && tx.webhook_url != "--" {
            let _ = db.webhook_log_insert(
                &tx.ref_,
                &tx.brand_id,
                &payload.to_string(),
                &tx.webhook_url,
                &now,
            );
            report.webhooks_queued += 1;
        }
    }

    // 2. Balance verification: longest-chain reconcile for active rows.
    let verifications = db.active_balance_verifications().unwrap_or_default();
    for (device_id, sender_key, type_) in &verifications {
        if db
            .reconcile_by_longest_chain(device_id, sender_key, type_, &now)
            .is_ok()
        {
            report.reconciled += 1;
        }
    }

    // 3. Deliver pending webhooks.
    let limit: i64 = db
        .env_get("geneal-application-settings-webhook_attempts_limit")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .max(1);
    let jobs = db.pending_webhooks(limit).unwrap_or_default();
    if !jobs.is_empty() {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(5))
            .build();
        if let Ok(client) = client {
            for job in &jobs {
                let _ = db.webhook_attempt_increment(job.id, &now);
                report.webhooks_sent += 1;
                let code: u16 = client
                    .post(&job.url)
                    .header("Content-Type", "application/json")
                    .body(job.payload.clone())
                    .send()
                    .await
                    .map(|r| r.status().as_u16())
                    .unwrap_or(0);
                let status = if code == 200 {
                    report.webhooks_completed += 1;
                    "completed"
                } else if job.attempts + 1 >= limit {
                    "canceled"
                } else {
                    "pending"
                };
                let _ = db.webhook_update_status(job.id, status, code, &now);
            }
        }
    }

    report
}

/// Run once if no run is in flight (10-minute lock, like PHP's
/// cron.lock). Returns None when a run is already going.
pub async fn try_run_cron(
    db: &Arc<Db>,
    lock: &Arc<Mutex<Option<Instant>>>,
) -> Option<CronReport> {
    {
        let mut guard = lock.lock().expect("cron lock");
        if let Some(started) = *guard {
            if started.elapsed() < Duration::from_secs(600) {
                return None;
            }
        }
        *guard = Some(Instant::now());
    }
    let report = run_cron(db).await;
    *lock.lock().expect("cron lock") = None;
    let now = now_utc();
    let _ = db.env_set("last-cron-invocation", &now, &now);
    Some(report)
}

/// Internal timer loop; started only when CRON_INTERVAL_SECS is set.
pub async fn scheduler_loop(
    db: Arc<Db>,
    lock: Arc<Mutex<Option<Instant>>>,
    interval_secs: u64,
) {
    loop {
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
        let _ = try_run_cron(&db, &lock).await;
    }
}

pub async fn cron_handler(
    State(state): State<AppState>,
    Path(secret): Path<String>,
) -> impl IntoResponse {
    let expected = state
        .db
        .env_get("cron-job")
        .ok()
        .flatten()
        .or_else(|| std::env::var("CRON_SECRET").ok());
    match expected {
        Some(e) if !e.is_empty() && e == secret => {}
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Not found"})),
            );
        }
    }
    let db = state.db.clone();
    let lock = state.cron_lock.clone();
    tokio::spawn(async move {
        let _ = try_run_cron(&db, &lock).await;
    });
    (
        StatusCode::OK,
        Json(json!({"status": "true", "message": "Cron run executed."})),
    )
}
