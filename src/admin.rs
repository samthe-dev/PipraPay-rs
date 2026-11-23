//! Admin panel: Askama server-rendered pages (+ a little HTMX).
//!
//! Auth mirrors PHP (`pp-adapter.php`): login by username/email +
//! bcrypt password (temp_password fallback), admin must be active and
//! have an active permission row for a brand, session cookie
//! `pp_admin` stored in `pp_browser_log` (32 hex chars), TOTP 2FA via
//! `pp_2fa` cookie when enabled, per-session CSRF token stored on the
//! session row. First run with no admin shows `/admin/setup`, which
//! creates the first admin + brand + BDT currency + full permission,
//! mirroring the PHP installer.
//!
//! Simplifications vs PHP, noted in STUDY.md: one brand per session
//! (the first active permission's brand; PHP also keys off a
//! `pp_brand` cookie), staff permission checks are role-based
//! (`admin` = full access), and the 89 POST actions are covered
//! incrementally — this module starts with dashboard, devices, SMS
//! data, transactions, brands and gateways.

use crate::db::{AdminRow, Db};
use crate::sms::money_to_string;
use crate::{format_user_time, generate_item_id_len, now_utc};
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Redirect},
    Form,
};
use serde_json::json;
use std::collections::HashMap;

use crate::AppState;

fn random_hex(n_bytes: usize) -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let bytes: Vec<u8> = (0..n_bytes).map(|_| rng.random()).collect();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn get_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=') {
            if k.trim() == name {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

fn set_cookie(name: &str, value: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax"))
        .expect("valid cookie")
}

fn clear_cookie(name: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("{name}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax"))
        .expect("valid cookie")
}

pub struct AdminSession {
    pub admin: AdminRow,
    pub brand_id: String,
    pub csrf_token: String,
    pub cookie: String,
}

pub fn resolve_session(headers: &HeaderMap, db: &Db) -> Option<AdminSession> {
    let cookie = get_cookie(headers, "pp_admin")?;
    let session = db.session_by_cookie(&cookie).ok().flatten()?;
    let admin = db.admin_by_a_id(&session.a_id).ok().flatten()?;
    if admin.status != "active" {
        return None;
    }
    let brand_id = db
        .first_permission_brand(&admin.a_id)
        .ok()
        .flatten()
        .or_else(|| {
            if admin.role == "admin" {
                db.first_brand_id().ok().flatten()
            } else {
                None
            }
        })?;
    Some(AdminSession {
        admin,
        brand_id,
        csrf_token: session.csrf_token,
        cookie: session.cookie,
    })
}

fn require_admin(
    headers: &HeaderMap,
    db: &Db,
) -> Result<AdminSession, Redirect> {
    resolve_session(headers, db).ok_or_else(|| Redirect::to("/admin/login"))
}

fn check_csrf(form: &HashMap<String, String>, session: &AdminSession) -> bool {
    form.get("csrf_token").map(String::as_str) == Some(session.csrf_token.as_str())
}

// ---------- base32 (for TOTP secrets) ----------

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut bits: u32 = 0;
    let mut nbits = 0;
    let mut out = Vec::new();
    for c in s.trim().trim_end_matches('=').chars() {
        let v = B32.iter().position(|&b| b as char == c.to_ascii_uppercase())? as u32;
        bits = (bits << 5) | v;
        nbits += 5;
        if nbits >= 8 {
            nbits -= 8;
            out.push((bits >> nbits) as u8);
        }
    }
    Some(out)
}

fn generate_totp_secret() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    (0..32)
        .map(|_| B32[rng.random_range(0..32)] as char)
        .collect()
}

/// PHP `verifyCode(secret, code, 2)`: allow ±2 time steps.
fn verify_totp(secret_b32: &str, code: &str) -> bool {
    let Some(secret) = base32_decode(secret_b32) else {
        return false;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for step in -2i64..=2 {
        let t = now.saturating_add_signed(step * 30);
        let expected = totp_lite::totp_custom::<totp_lite::Sha1>(30, 6, &secret, t);
        if expected == code {
            return true;
        }
    }
    false
}

// ---------- templates ----------

#[derive(Template)]
#[template(path = "admin/base.html")]
struct BaseTemplate {
    title: String,
    admin_name: String,
    brand_id: String,
}

#[derive(Template)]
#[template(path = "admin/login.html")]
struct LoginTemplate {
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "admin/setup.html")]
struct SetupTemplate {
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "admin/twofa.html")]
struct TwoFaTemplate {
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "admin/dashboard.html")]
struct DashboardTemplate {
    title: String,
    admin_name: String,
    brand_id: String,
    tx_counts: Vec<(String, i64)>,
    sms_counts: Vec<(String, i64)>,
    device_count: i64,
    brand_count: i64,
    recent: Vec<TxView>,
}

#[derive(Template)]
#[template(path = "admin/devices.html")]
struct DevicesTemplate {
    title: String,
    admin_name: String,
    brand_id: String,
    csrf_token: String,
    devices: Vec<DeviceView>,
    qr: Option<QrView>,
    site_url: String,
}

#[derive(Template)]
#[template(path = "admin/sms.html")]
struct SmsTemplate {
    title: String,
    admin_name: String,
    brand_id: String,
    csrf_token: String,
    filter: String,
    rows: Vec<SmsView>,
}

#[derive(Template)]
#[template(path = "admin/transactions.html")]
struct TransactionsTemplate {
    title: String,
    admin_name: String,
    brand_id: String,
    filter: String,
    rows: Vec<TxView>,
}

#[derive(Template)]
#[template(path = "admin/brands.html")]
struct BrandsTemplate {
    title: String,
    admin_name: String,
    brand_id: String,
    csrf_token: String,
    brands: Vec<crate::db::BrandRow>,
}

#[derive(Template)]
#[template(path = "admin/gateways.html")]
struct GatewaysTemplate {
    title: String,
    admin_name: String,
    brand_id: String,
    gateways: Vec<crate::db::GatewayListRow>,
}

pub struct TxView {
    pub ref_: String,
    pub customer: String,
    pub amount: String,
    pub currency: String,
    pub status: String,
    pub created: String,
}

pub struct DeviceView {
    pub id: i64,
    pub device_id: String,
    pub name: String,
    pub model: String,
    pub status: String,
    pub last_sync: String,
    pub created: String,
}

pub struct QrView {
    pub device_id: String,
    pub otp: String,
    pub qr_text: String,
}

pub struct SmsView {
    pub id: i64,
    pub sender: String,
    pub message: String,
    pub reason: String,
    pub status: String,
    pub created: String,
}

fn tx_view(row: &crate::db::TransactionRow) -> TxView {
    let customer: serde_json::Value =
        serde_json::from_str(&row.customer_info).unwrap_or(json!({}));
    TxView {
        ref_: row.ref_.clone(),
        customer: customer["name"].as_str().unwrap_or("N/A").to_string(),
        amount: money_to_string(row.amount),
        currency: row.currency.clone(),
        status: row.status.clone(),
        created: format_user_time(&row.created_date),
    }
}

fn login_failed(msg: &str) -> Html<String> {
    Html(
        LoginTemplate {
            error: Some(msg.to_string()),
        }
        .render()
        .unwrap_or_default(),
    )
}

// ---------- setup ----------

pub async fn setup_get(State(state): State<AppState>) -> impl IntoResponse {
    if state.db.admin_count().unwrap_or(0) > 0 {
        return Redirect::to("/admin/login").into_response();
    }
    Html(
        SetupTemplate { error: None }
            .render()
            .unwrap_or_default(),
    )
    .into_response()
}

pub async fn setup_post(
    State(state): State<AppState>,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    if state.db.admin_count().unwrap_or(0) > 0 {
        return Redirect::to("/admin/login").into_response();
    }
    let full_name = form.get("full_name").cloned().unwrap_or_default();
    let username = form.get("username").cloned().unwrap_or_default();
    let email = form.get("email").cloned().unwrap_or_default();
    let password = form.get("password").cloned().unwrap_or_default();
    let brand_name = form.get("brand_name").cloned().unwrap_or_default();
    if full_name.is_empty() || username.is_empty() || email.is_empty()
        || password.len() < 8 || brand_name.is_empty()
    {
        return Html(
            SetupTemplate {
                error: Some(
                    "All fields are required and the password must be at least 8 characters."
                        .to_string(),
                ),
            }
            .render()
            .unwrap_or_default(),
        )
        .into_response();
    }
    let hash = match bcrypt::hash(&password, bcrypt::DEFAULT_COST) {
        Ok(h) => h,
        Err(_) => {
            return Html(
                SetupTemplate {
                    error: Some("Could not hash the password.".to_string()),
                }
                .render()
                .unwrap_or_default(),
            )
            .into_response();
        }
    };
    let now = now_utc();
    let a_id = generate_item_id_len(10);
    let brand_id = generate_item_id_len(10);
    let ok = state.db
        .create_admin(&a_id, &full_name, &username, &email, &hash, &now)
        .and_then(|_| state.db.create_brand(&brand_id, &brand_name, &email, &now))
        .and_then(|_| state.db.create_currency(&brand_id, "BDT"))
        .and_then(|_| {
            state.db.create_permission(
                &a_id,
                &brand_id,
                r#"{"*":true}"#,
                &now,
            )
        })
        .is_ok();
    if !ok {
        return Html(
            SetupTemplate {
                error: Some("Setup failed — the admin or brand may already exist.".to_string()),
            }
            .render()
            .unwrap_or_default(),
        )
        .into_response();
    }
    Redirect::to("/admin/login").into_response()
}

// ---------- login / 2fa / logout ----------

pub async fn login_get() -> Html<String> {
    Html(LoginTemplate { error: None }.render().unwrap_or_default())
}

pub async fn login_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let _ = headers;
    let login = form.get("username").cloned().unwrap_or_default();
    let password = form.get("password").cloned().unwrap_or_default();
    if login.is_empty() || password.is_empty() {
        return login_failed("Please fill in all required fields before proceeding.")
            .into_response();
    }
    let Some(admin) = state.db.admin_by_login(&login).ok().flatten() else {
        return login_failed("The email or password you entered is incorrect.")
            .into_response();
    };
    let password_ok = bcrypt::verify(&password, &admin.password).unwrap_or(false)
        || admin
            .temp_password
            .as_deref()
            .map(|t| bcrypt::verify(&password, t).unwrap_or(false))
            .unwrap_or(false);
    if !password_ok {
        return login_failed("The email or password you entered is incorrect.")
            .into_response();
    }
    if admin.status != "active" {
        return login_failed("Your account has been suspended. Please contact with your admin.")
            .into_response();
    }
    // PHP generates a 2FA secret on login when none is set.
    if admin.twofa_secret == "--" || admin.twofa_secret.is_empty() {
        let _ = state.db.set_twofa_secret(&admin.a_id, &generate_totp_secret());
    }
    let brand_id = match state.db.first_permission_brand(&admin.a_id).ok().flatten() {
        Some(b) => b,
        None => {
            return login_failed("You don’t have permission to manage brands. Contact your admin.")
                .into_response();
        }
    };
    let cookie = random_hex(16);
    let csrf = random_hex(32);
    let now = now_utc();
    if state.db
        .create_browser_log(&admin.a_id, &cookie, &csrf, &now)
        .is_err()
    {
        return login_failed("Your request could not be processed. Please try again.")
            .into_response();
    }
    let mut resp_headers = HeaderMap::new();
    resp_headers.append(axum::http::header::SET_COOKIE, set_cookie("pp_brand", &brand_id));
    if admin.twofa_status == "enable" {
        resp_headers.append(axum::http::header::SET_COOKIE, set_cookie("pp_2fa", &cookie));
        (resp_headers, Redirect::to("/admin/2fa")).into_response()
    } else {
        resp_headers.append(axum::http::header::SET_COOKIE, set_cookie("pp_admin", &cookie));
        (resp_headers, Redirect::to("/admin/dashboard")).into_response()
    }
}

pub async fn twofa_get(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(cookie) = get_cookie(&headers, "pp_2fa") else {
        return Redirect::to("/admin/login").into_response();
    };
    match state.db.session_by_cookie(&cookie).ok().flatten() {
        Some(_) => Html(
            TwoFaTemplate { error: None }
                .render()
                .unwrap_or_default(),
        )
        .into_response(),
        None => Redirect::to("/admin/login").into_response(),
    }
}

pub async fn twofa_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(cookie) = get_cookie(&headers, "pp_2fa") else {
        return Redirect::to("/admin/login").into_response();
    };
    let Some(session) = state.db.session_by_cookie(&cookie).ok().flatten() else {
        return Redirect::to("/admin/login").into_response();
    };
    let Some(admin) = state.db.admin_by_a_id(&session.a_id).ok().flatten() else {
        return Redirect::to("/admin/login").into_response();
    };
    let code = form.get("code").cloned().unwrap_or_default();
    if code.len() != 6 || !verify_totp(&admin.twofa_secret, &code) {
        return Html(
            TwoFaTemplate {
                error: Some("The code you entered is incorrect. Please try again.".to_string()),
            }
            .render()
            .unwrap_or_default(),
        )
        .into_response();
    }
    let mut resp_headers = HeaderMap::new();
    resp_headers.append(axum::http::header::SET_COOKIE, clear_cookie("pp_2fa"));
    resp_headers.append(axum::http::header::SET_COOKIE, set_cookie("pp_admin", &cookie));
    (resp_headers, Redirect::to("/admin/dashboard")).into_response()
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if let Some(cookie) = get_cookie(&headers, "pp_admin") {
        let _ = state.db.expire_browser_log(&cookie);
    }
    let mut resp_headers = HeaderMap::new();
    resp_headers.append(axum::http::header::SET_COOKIE, clear_cookie("pp_admin"));
    resp_headers.append(axum::http::header::SET_COOKIE, clear_cookie("pp_2fa"));
    resp_headers.append(axum::http::header::SET_COOKIE, clear_cookie("pp_brand"));
    (resp_headers, Redirect::to("/admin/login")).into_response()
}

// ---------- pages ----------

pub async fn dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Ok(session) = require_admin(&headers, &state.db) else {
        return Redirect::to("/admin/login").into_response();
    };
    let tx_counts = state.db
        .count_by_status("pp_transaction", "status", &format!("WHERE brand_id = '{}'", session.brand_id))
        .unwrap_or_default();
    let sms_counts = state.db
        .count_by_status("pp_sms_data", "status", "")
        .unwrap_or_default();
    let device_count = state.db
        .count_by_status("pp_device", "status", "")
        .unwrap_or_default()
        .iter()
        .map(|(_, c)| c)
        .sum();
    let brand_count = state.db.list_brands().map(|b| b.len() as i64).unwrap_or(0);
    let recent = state.db
        .list_transactions(&session.brand_id, None)
        .unwrap_or_default()
        .into_iter()
        .take(10)
        .map(|r| tx_view(&r))
        .collect();
    Html(
        DashboardTemplate {
            title: "Dashboard".into(),
            admin_name: session.admin.full_name.clone(),
            brand_id: session.brand_id.clone(),
            tx_counts,
            sms_counts,
            device_count,
            brand_count,
            recent,
        }
        .render()
        .unwrap_or_default(),
    )
    .into_response()
}

pub async fn devices_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let Ok(session) = require_admin(&headers, &state.db) else {
        return Redirect::to("/admin/login").into_response();
    };
    let devices = state.db
        .list_devices()
        .unwrap_or_default()
        .into_iter()
        .map(|d| DeviceView {
            id: d.id,
            device_id: d.device_id,
            name: if d.name.is_empty() { "--".into() } else { d.name },
            model: if d.model.is_empty() { "--".into() } else { d.model },
            status: d.status,
            last_sync: d.last_sync,
            created: format_user_time(&d.created_date),
        })
        .collect();
    let qr = query.get("connected").and_then(|_| {
        // After a connect POST we redirect with ?connected=<device_id>:<otp>.
        query.get("qr").map(|q| {
            let mut parts = q.splitn(2, ':');
            let device_id = parts.next().unwrap_or("").to_string();
            let otp = parts.next().unwrap_or("").to_string();
            let site_url = query.get("site").cloned().unwrap_or_default();
            QrView {
                qr_text: format!("{site_url}----{otp}"),
                device_id,
                otp,
            }
        })
    });
    Html(
        DevicesTemplate {
            title: "Devices".into(),
            admin_name: session.admin.full_name.clone(),
            brand_id: session.brand_id.clone(),
            csrf_token: session.csrf_token.clone(),
            devices,
            qr,
            site_url: state.site_url.clone(),
        }
        .render()
        .unwrap_or_default(),
    )
    .into_response()
}

pub async fn devices_connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let Ok(session) = require_admin(&headers, &state.db) else {
        return Redirect::to("/admin/login").into_response();
    };
    if !check_csrf(&form, &session) {
        return (StatusCode::FORBIDDEN, "Invalid CSRF token").into_response();
    }
    let now = now_utc();
    let (device_id, otp) = match state.db.device_connect_info(&session.cookie, &now) {
        Ok(v) => v,
        Err(_) => return Redirect::to("/admin/devices").into_response(),
    };
    // QR text the Android app scans: {baseUrl}----{otp}.
    let site_url = state.site_url.clone();
    Redirect::to(&format!(
        "/admin/devices?connected=1&qr={device_id}:{otp}&site={site_url}"
    ))
    .into_response()
}

pub async fn devices_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let Ok(session) = require_admin(&headers, &state.db) else {
        return Redirect::to("/admin/login").into_response();
    };
    if !check_csrf(&form, &session) {
        return (StatusCode::FORBIDDEN, "Invalid CSRF token").into_response();
    }
    let _ = state.db.delete_device(id);
    Redirect::to("/admin/devices").into_response()
}

pub async fn sms_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let Ok(session) = require_admin(&headers, &state.db) else {
        return Redirect::to("/admin/login").into_response();
    };
    let filter = query.get("status").cloned().unwrap_or_default();
    let rows = state.db
        .list_sms_data(if filter.is_empty() { None } else { Some(filter.as_str()) })
        .unwrap_or_default()
        .into_iter()
        .map(|r| SmsView {
            id: r.id,
            sender: r.sender,
            message: r.message,
            reason: r.reason,
            status: r.status,
            created: format_user_time(&r.created_date),
        })
        .collect();
    Html(
        SmsTemplate {
            title: "SMS Data".into(),
            admin_name: session.admin.full_name.clone(),
            brand_id: session.brand_id.clone(),
            csrf_token: session.csrf_token.clone(),
            filter,
            rows,
        }
        .render()
        .unwrap_or_default(),
    )
    .into_response()
}

pub async fn sms_approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let Ok(session) = require_admin(&headers, &state.db) else {
        return Redirect::to("/admin/login").into_response();
    };
    if !check_csrf(&form, &session) {
        return (StatusCode::FORBIDDEN, "Invalid CSRF token").into_response();
    }
    let _ = state.db.sms_set_status(id, "approved", &now_utc());
    Redirect::to("/admin/sms?status=awaiting-review").into_response()
}

pub async fn sms_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let Ok(session) = require_admin(&headers, &state.db) else {
        return Redirect::to("/admin/login").into_response();
    };
    if !check_csrf(&form, &session) {
        return (StatusCode::FORBIDDEN, "Invalid CSRF token").into_response();
    }
    let _ = state.db.delete_sms(id);
    Redirect::to("/admin/sms").into_response()
}

pub async fn transactions_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let Ok(session) = require_admin(&headers, &state.db) else {
        return Redirect::to("/admin/login").into_response();
    };
    let filter = query.get("status").cloned().unwrap_or_default();
    let rows = state.db
        .list_transactions(
            &session.brand_id,
            if filter.is_empty() { None } else { Some(filter.as_str()) },
        )
        .unwrap_or_default()
        .into_iter()
        .map(|r| tx_view(&r))
        .collect();
    Html(
        TransactionsTemplate {
            title: "Transactions".into(),
            admin_name: session.admin.full_name.clone(),
            brand_id: session.brand_id.clone(),
            filter,
            rows,
        }
        .render()
        .unwrap_or_default(),
    )
    .into_response()
}

pub async fn brands_get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Ok(session) = require_admin(&headers, &state.db) else {
        return Redirect::to("/admin/login").into_response();
    };
    let brands = state.db.list_brands().unwrap_or_default();
    Html(
        BrandsTemplate {
            title: "Brands".into(),
            admin_name: session.admin.full_name.clone(),
            brand_id: session.brand_id.clone(),
            csrf_token: session.csrf_token.clone(),
            brands,
        }
        .render()
        .unwrap_or_default(),
    )
    .into_response()
}

pub async fn brands_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let Ok(session) = require_admin(&headers, &state.db) else {
        return Redirect::to("/admin/login").into_response();
    };
    if !check_csrf(&form, &session) {
        return (StatusCode::FORBIDDEN, "Invalid CSRF token").into_response();
    }
    let name = form.get("name").cloned().unwrap_or_default();
    let email = form.get("email").cloned().unwrap_or_default();
    if !name.is_empty() {
        let brand_id = generate_item_id_len(10);
        let now = now_utc();
        let _ = state.db.create_brand(&brand_id, &name, &email, &now);
        let _ = state.db.create_currency(&brand_id, "BDT");
    }
    Redirect::to("/admin/brands").into_response()
}

pub async fn gateways_get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Ok(session) = require_admin(&headers, &state.db) else {
        return Redirect::to("/admin/login").into_response();
    };
    let gateways = state.db.list_gateways(&session.brand_id).unwrap_or_default();
    Html(
        GatewaysTemplate {
            title: "Gateways".into(),
            admin_name: session.admin.full_name.clone(),
            brand_id: session.brand_id.clone(),
            gateways,
        }
        .render()
        .unwrap_or_default(),
    )
    .into_response()
}
