mod db;
mod sms;

use axum::{
    Form, Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use rand::Rng;
use serde_json::{Value, json};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};

#[derive(Clone)]
struct AppState {
    db: Arc<db::Db>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "piprapay_rs=info".into()),
        )
        .init();

    let database_path =
        std::env::var("DATABASE_PATH").unwrap_or_else(|_| "piprapay.db".into());
    let listen_addr: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()
        .expect("LISTEN_ADDR must be a socket address");

    let state = AppState {
        db: Arc::new(db::Db::open(&database_path).expect("open SQLite database")),
    };
    tracing::info!(database_path, %listen_addr, "piprapay-rs starting");

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/", post(companion))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .expect("bind listen address");
    axum::serve(listener, app).await.expect("serve");
}

async fn healthz() -> Json<Value> {
    Json(json!({"status": "ok", "service": "piprapay-rs", "version": env!("CARGO_PKG_VERSION")}))
}

/// Current UTC time in PHP's `Y-m-d H:i:s` format.
fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// PHP `generateItemID()`: 10 random digits (mt_rand 0-9 per digit).
fn generate_item_id() -> String {
    let mut rng = rand::rng();
    (0..10)
        .map(|_| rng.random_range(0..=9).to_string())
        .collect()
}

fn err(title: &str, message: &str) -> Value {
    json!({"status": "false", "title": title, "message": message})
}

/// Companion API: POST / with form field `action-companion`.
/// This is the Android app's only server surface (see STUDY.md section 2).
async fn companion(
    State(state): State<AppState>,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let action = form.get("action-companion").cloned().unwrap_or_default();
    let reply = match action.as_str() {
        "login" => companion_login(&state, &form),
        "account-information" => json!({"status": "false", "title": "Not implemented", "message": "account-information is not implemented yet"}),
        "sms-transmit-sender" => companion_senders(&state, &form),
        "sms-transmit-bulk" => companion_bulk(&state, &form),
        "delete-sms-data" => json!({"status": "false", "title": "Not implemented", "message": "delete-sms-data is not implemented yet"}),
        "" => err("Oops! Something went wrong", "Your request could not be processed. Please try again."),
        _ => err("Oops! Something went wrong", "Your request could not be processed. Please try again."),
    };
    (StatusCode::OK, Json(reply))
}

/// PHP `login`: look up `pp_device` by OTP with no status filter, then
/// rotate the OTP to a fresh 10-digit ID, fill in device details and set
/// status `used`. Returns `{"status":"true","token":<new otp>}`.
fn companion_login(state: &AppState, form: &HashMap<String, String>) -> Value {
    let otp = form.get("onetimepassword").cloned().unwrap_or_default();
    if otp.is_empty() {
        return err(
            "Incomplete Information",
            "Please fill in all required fields before proceeding.",
        );
    }
    let Some(device) = state.db.device_by_otp(&otp).ok().flatten() else {
        return err(
            "Invalid Credentials",
            "Please enter the correct credentials or scan the QR code again.",
        );
    };
    let new_otp = generate_item_id();
    let now = now_utc();
    if state
        .db
        .login_device(
            device.id,
            &new_otp,
            form.get("name").map(String::as_str).unwrap_or(""),
            form.get("model").map(String::as_str).unwrap_or(""),
            form.get("android_level").map(String::as_str).unwrap_or(""),
            form.get("app_version").map(String::as_str).unwrap_or(""),
            &now,
        )
        .is_err()
    {
        return err(
            "Oops! Something went wrong",
            "Your request could not be processed. Please try again.",
        );
    }
    json!({"status": "true", "token": new_otp})
}

/// PHP `sms-transmit-sender`: auth by token, then the exact whitelist.
fn companion_senders(state: &AppState, form: &HashMap<String, String>) -> Value {
    let token = form.get("token").cloned().unwrap_or_default();
    if token.is_empty() {
        return err(
            "Incomplete Information",
            "Please fill in all required fields before proceeding.",
        );
    }
    match state.db.device_by_token(&token) {
        Ok(Some(_)) => json!({"status": "true", "senders": sms::SENDERS}),
        _ => err(
            "Authentication Failed",
            "Please try again or scan the QR code again.",
        ),
    }
}

/// PHP `sms-transmit-bulk`, with one deliberate fix: PHP echoes a
/// separate JSON object per SMS (and `exit()`s on Code 102), which the
/// app cannot parse for more than one item. Rust returns ONE JSON object
/// for the whole bulk, with a per-item result for every SMS.
fn companion_bulk(state: &AppState, form: &HashMap<String, String>) -> Value {
    let token = form.get("token").cloned().unwrap_or_default();
    if token.is_empty() {
        return err(
            "Incomplete Information",
            "Please fill in all required fields before proceeding.",
        );
    }
    let Some(device) = state.db.device_by_token(&token).ok().flatten() else {
        return err(
            "Authentication Failed",
            "Please try again or scan the QR code again.",
        );
    };
    let raw = form.get("sms_list").cloned().unwrap_or_default();
    let list: Vec<Value> = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => {
            return err(
                "Invalid SMS list",
                "Please fill in all required fields before proceeding.",
            );
        }
    };

    let mut results: Vec<Value> = Vec::new();
    for item in &list {
        results.push(process_one_sms(state, &device, item));
    }
    let created = results
        .iter()
        .filter(|r| r["status"] == "true")
        .count();
    json!({
        "status": "true",
        "title": "SMS Data Created",
        "message": format!("Processed {} SMS, {} stored.", list.len(), created),
        "results": results,
    })
}

/// One SMS through the PHP bulk logic. Returns the per-item result that
/// goes into the bulk response's `results` array.
fn process_one_sms(state: &AppState, device: &db::DeviceRow, item: &Value) -> Value {
    let now = now_utc();
    let id = item["id"].as_str().unwrap_or("").trim().to_string();
    let sender = item["sender"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let message = item["message"].as_str().unwrap_or("").trim().to_string();
    let simslot = item["simSlot"].as_str().unwrap_or("").trim().to_string();
    // Android sends simSlot as a number in some builds; accept both.
    let simslot = if simslot.is_empty() {
        item["simSlot"]
            .as_i64()
            .map(|n| n.to_string())
            .unwrap_or_default()
    } else {
        simslot
    };

    let touch = |state: &AppState| {
        let _ = state.db.touch_device_last_sync(device.id, &now);
    };

    let (sender_key, currency, balance_verify) = match sms::sender_whitelist(&sender) {
        Some((key, _name, currency, balance_verify)) => (key.to_string(), currency.to_string(), balance_verify.to_string()),
        None => ("--".to_string(), "--".to_string(), "--".to_string()),
    };

    let Some(parsed) = sms::parse_message(&sender_key, &message) else {
        let _ = state.db.insert_sms_error(
            &device.device_id,
            &sender,
            &simslot,
            "error",
            &message,
            "Invalid or unknown message. Code 101",
            &now,
        );
        touch(state);
        return json!({"id": id, "status": "false", "code": 101, "title": "Invalid or unknown MFS message", "message": "Please fill in all required fields before proceeding."});
    };

    if parsed.type_.is_empty()
        || parsed.amount.is_empty()
        || parsed.sender.is_empty()
        || parsed.trxid.is_empty()
    {
        let _ = state.db.insert_sms_error(
            &device.device_id,
            &sender,
            &simslot,
            "error",
            &message,
            "Invalid or unknown message. Code 102",
            &now,
        );
        touch(state);
        // PHP exit()s here, killing the rest of the bulk; Rust continues
        // with the next SMS so one bad item cannot lose the whole batch.
        return json!({"id": id, "status": "false", "code": 102, "title": "Invalid or unknown MFS message", "message": "Please fill in all required fields before proceeding."});
    }

    if state
        .db
        .sms_exists(&sender_key, &parsed.trxid)
        .unwrap_or(false)
    {
        let _ = state.db.insert_sms_error(
            &device.device_id,
            &sender,
            &simslot,
            "error",
            &message,
            "Duplicate message. Code 103",
            &now,
        );
        touch(state);
        return json!({"id": id, "status": "false", "code": 103, "title": "Duplicate Transaction", "message": "The provided Transaction ID already exists in our system.", "trxid": parsed.trxid});
    }

    let balance_str = parsed.balance.clone().unwrap_or_else(|| "--".to_string());

    // cellfin has balance_verify='false': approve without a balance check.
    if balance_verify == "false" {
        let row = state.db.insert_sms_data(
            "app",
            &device.device_id,
            &sender,
            &sender_key,
            &simslot,
            &parsed.sender,
            parsed.amount_minor,
            &currency,
            &parsed.trxid,
            &balance_str,
            &parsed.type_,
            "approved",
            &message,
            "--",
            &now,
        );
        touch(state);
        return match row {
            Ok(row_id) => json!({"id": id, "row_id": row_id, "status": "true", "title": "SMS Data Created", "message": "The sms data has been created successfully.", "trxid": parsed.trxid, "sms_status": "approved"}),
            Err(_) => err("Oops! Something went wrong", "Your request could not be processed. Please try again."),
        };
    }

    // Balance verification: only when a verification row exists and is active.
    let bv = state
        .db
        .balance_verification(&device.device_id, &sender_key, &parsed.type_)
        .ok()
        .flatten();
    let Some(bv) = bv.filter(|b| b.status == "active") else {
        let row = state.db.insert_sms_data(
            "app",
            &device.device_id,
            &sender,
            &sender_key,
            &simslot,
            &parsed.sender,
            parsed.amount_minor,
            &currency,
            &parsed.trxid,
            &balance_str,
            &parsed.type_,
            "approved",
            &message,
            "--",
            &now,
        );
        touch(state);
        return match row {
            Ok(row_id) => json!({"id": id, "row_id": row_id, "status": "true", "title": "SMS Data Created", "message": "The sms data has been created successfully.", "trxid": parsed.trxid, "sms_status": "approved"}),
            Err(_) => err("Oops! Something went wrong", "Your request could not be processed. Please try again."),
        };
    };

    let bsimslot = if simslot == "1" { "Sim1" } else { "Sim2" };
    let expected = bv.current_balance + parsed.amount_minor;
    let balance_minor = parsed.balance_minor.unwrap_or(0);
    let (sms_status, reason) = if expected == balance_minor {
        if bv.simslot != "Any" && bv.simslot != bsimslot {
            (
                "awaiting-review".to_string(),
                format!("SIM slot and expected slot do not match. Recorded: {bsimslot}; Expected: {}", bv.simslot),
            )
        } else {
            let _ = state
                .db
                .update_balance_verification(bv.id, expected, &now);
            ("approved".to_string(), "--".to_string())
        }
    } else {
        let mut reasons = vec![format!(
            "SMS balance and expected balance do not match. Recorded SMS balance: {}; Expected balance: {}",
            sms::money_to_string(balance_minor),
            sms::money_to_string(expected)
        )];
        if bv.simslot != "Any" && bv.simslot != bsimslot {
            reasons.push(format!(
                "SIM slot and expected slot do not match. Recorded: {bsimslot}; Expected: {}",
                bv.simslot
            ));
        }
        ("awaiting-review".to_string(), reasons.join(" | "))
    };

    let row = state.db.insert_sms_data(
        "app",
        &device.device_id,
        &sender,
        &sender_key,
        &simslot,
        &parsed.sender,
        parsed.amount_minor,
        &currency,
        &parsed.trxid,
        &balance_str,
        &parsed.type_,
        &sms_status,
        &message,
        &reason,
        &now,
    );
    touch(state);
    if sms_status == "awaiting-review" {
        let _ = state.db.reconcile_by_longest_chain(
            &device.device_id,
            &sender_key,
            &parsed.type_,
            &now,
        );
    }
    match row {
        Ok(row_id) => json!({"id": id, "row_id": row_id, "status": "true", "title": "SMS Data Created", "message": "The sms data has been created successfully.", "trxid": parsed.trxid, "sms_status": sms_status, "reason": reason}),
        Err(_) => err("Oops! Something went wrong", "Your request could not be processed. Please try again."),
    }
}
