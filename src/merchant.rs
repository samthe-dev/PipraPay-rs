//! Merchant API: `POST /api/checkout/redirect`, `/api/checkout/popup`,
//! `/api/verify-payment`, `/api/refund-payment`.
//!
//! Faithful port of the `case 'api'` branch in PHP `index.php:332-1027`:
//! auth by `MHS-PIPRAPAY-API-KEY` header, active key, expiry, scopes
//! (`create_payment`, `verify_payment`, `refund_payment`), the same
//! validation order and the same `400 {error:{code,message}}` shape.
//!
//! Deliberate limits (see STUDY.md): the checkout *page* itself
//! (`/payment/<pp_id>`, theme rendering) and API gateways are not in
//! Phase 1. A refund on a `bkash-api-tokenized` transaction therefore
//! fails with REFUND_FAILED until that gateway lands; every other
//! gateway refunds locally, exactly as PHP does when no gateway refund
//! applies.

use crate::db::{ApiKeyRow, Db, TransactionRow};
use crate::sms::{money_to_string, parse_money};
use crate::{format_user_time, generate_item_id_len, now_utc};
use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};
use std::sync::Arc;

type Reply = (StatusCode, Json<Value>);

fn api_error(code: &str, message: &str) -> Reply {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": {"code": code, "message": message}})),
    )
}

fn insufficient_scope(scope: &str) -> Reply {
    api_error(
        "INSUFFICIENT_SCOPE",
        &format!("The API key does not have the required permission: {scope}"),
    )
}

/// PHP `isExpired()`: '--' or empty never expires; a date-only value
/// expires at end of that day.
fn is_expired(expired_date: &Option<String>) -> bool {
    let Some(v) = expired_date else {
        return false;
    };
    let v = v.trim();
    if v.is_empty() || v == "--" {
        return false;
    }
    use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};
    let ts = if NaiveDateTime::parse_from_str(v, "%Y-%m-%d %H:%M:%S").is_ok() {
        NaiveDateTime::parse_from_str(v, "%Y-%m-%d %H:%M:%S")
            .ok()
            .map(|dt| Utc.from_utc_datetime(&dt))
    } else if let Ok(d) = NaiveDate::parse_from_str(v, "%Y-%m-%d") {
        // End of day, like PHP's strtotime('Y-m-d 23:59:59').
        d.and_hms_opt(23, 59, 59).map(|dt| Utc.from_utc_datetime(&dt))
    } else {
        // Unparseable date: PHP treats strtotime() === false as expired.
        return true;
    };
    match ts {
        Some(t) => Utc::now() > t,
        None => true,
    }
}

/// Authenticate the API key and check one scope. Returns the key row on
/// success, or the 400 reply to send.
fn auth(headers: &HeaderMap, db: &Db, scope: &str) -> Result<ApiKeyRow, Reply> {
    let key = headers
        .get("mhs-piprapay-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_string();
    let Some(row) = db.api_key_lookup(&key).ok().flatten() else {
        return Err(api_error(
            "INVALID_API_KEY",
            "The API key provided is incorrect or invalid.",
        ));
    };
    if is_expired(&row.expired_date) {
        return Err(api_error(
            "INVALID_API_KEY",
            "The API key provided is incorrect or expired.",
        ));
    }
    let scopes: Vec<String> =
        serde_json::from_str(&row.api_scopes).unwrap_or_default();
    if !scopes.iter().any(|s| s == scope) {
        let label = match scope {
            "create_payment" => "Create Payment",
            "verify_payment" => "Verify Payment",
            "refund_payment" => "Refund Payment",
            _ => scope,
        };
        return Err(insufficient_scope(label));
    }
    Ok(row)
}

/// Parse the JSON body the way PHP does: invalid JSON, or an empty
/// payload, is INVALID_JSON_PAYLOAD.
fn parse_body(body: Bytes) -> Result<Value, Reply> {
    let data: Value = serde_json::from_slice(&body)
        .map_err(|_| api_error("INVALID_JSON_PAYLOAD", "The JSON payload is invalid or malformed."))?;
    match &data {
        Value::Object(map) if !map.is_empty() => Ok(data),
        _ => Err(api_error(
            "INVALID_JSON_PAYLOAD",
            "The JSON payload is invalid or malformed.",
        )),
    }
}

/// Host part of a URL, or None when it is not a valid http(s) URL.
fn domain_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    if host.is_empty() || host.contains(' ') {
        None
    } else {
        Some(host.to_lowercase())
    }
}

/// Validate a return/webhook URL against the brand's domain whitelist,
/// in PHP's order: invalid URL, not whitelisted, whitelisted but inactive.
/// Returns Ok("--") for an empty URL, Ok(url) when allowed.
fn check_url(db: &Db, url: &str, label: &str) -> Result<String, Reply> {
    if url.is_empty() {
        return Ok("--".to_string());
    }
    let Some(domain) = domain_of(url) else {
        return Err(api_error("INVALID_URL", &format!("{label} URL is invalid.")));
    };
    match db.domain_status(&domain).ok().flatten() {
        Some(status) if status == "active" => Ok(url.to_string()),
        Some(_) => Err(api_error(
            "INVALID_URL",
            &format!(
                "The {label} URL (\"{domain}\") is whitelisted but not active. Please activate this domain in the \"Domains\" section to proceed."
            ),
        )),
        None => Err(api_error(
            "INVALID_URL",
            &format!(
                "The provided {label} URL (\"{domain}\") is not whitelisted. Please add this domain in the \"Domains\" section to continue."
            ),
        )),
    }
}

/// `metadata` may be an object/array or a JSON string; anything else is
/// INVALID_METADATA, an unparseable string is INVALID_JSON.
fn parse_metadata(raw: Option<&Value>) -> Result<Value, Reply> {
    match raw {
        None => Ok(json!({})),
        Some(Value::String(s)) => serde_json::from_str(s)
            .map_err(|_| api_error("INVALID_JSON", "The metadata JSON is invalid.")),
        Some(v @ (Value::Object(_) | Value::Array(_))) => Ok(v.clone()),
        Some(_) => Err(api_error(
            "INVALID_METADATA",
            "Metadata must be an array or valid JSON string.",
        )),
    }
}

fn valid_email(email: &str) -> bool {
    if email.contains(' ') || email.len() < 5 {
        return false;
    }
    let mut parts = email.split('@');
    let (Some(_local), Some(domain)) = (parts.next(), parts.next()) else {
        return false;
    };
    parts.next().is_none() && domain.contains('.') && !domain.starts_with('.')
}

/// Amount may arrive as a JSON number or a numeric string; must be > 0.
fn parse_amount(raw: Option<&Value>) -> Option<i64> {
    let minor = match raw? {
        Value::Number(n) => parse_money(&n.to_string()),
        Value::String(s) => parse_money(s),
        _ => return None,
    };
    if minor <= 0 {
        None
    } else {
        Some(minor)
    }
}

struct CheckoutInput {
    full_name: String,
    email: String,
    mobile: String,
    amount_minor: i64,
    currency: String,
    metadata: Value,
}

/// Shared field validation for both checkout types, in PHP's order.
fn validate_checkout(data: &Value) -> Result<CheckoutInput, Reply> {
    let metadata = parse_metadata(data.get("metadata"))?;
    let full_name = data["full_name"].as_str().unwrap_or("").to_string();
    if full_name.is_empty() {
        return Err(api_error("MISSING_FIELD", "Full name is required."));
    }
    let email = data["email_address"].as_str().unwrap_or("").to_string();
    if email.is_empty() || !valid_email(&email) {
        return Err(api_error(
            "INVALID_EMAIL",
            "A valid email address is required.",
        ));
    }
    let mobile = data["mobile_number"].as_str().unwrap_or("").to_string();
    if mobile.is_empty() {
        return Err(api_error("MISSING_FIELD", "Mobile number is required."));
    }
    let Some(amount_minor) = parse_amount(data.get("amount")) else {
        return Err(api_error(
            "INVALID_AMOUNT",
            "Amount must be a positive number.",
        ));
    };
    let currency = data["currency"].as_str().unwrap_or("BDT").to_string();
    Ok(CheckoutInput {
        full_name,
        email,
        mobile,
        amount_minor,
        currency,
        metadata,
    })
}

fn finish_checkout(
    db: &Db,
    key: &ApiKeyRow,
    input: CheckoutInput,
    return_url: Option<String>,
    webhook_url: String,
    site_url: &str,
) -> Reply {
    if !db
        .currency_exists(&key.brand_id, &input.currency)
        .unwrap_or(false)
    {
        return api_error("INVALID_CURRENCY", "Currency not supported.");
    }
    if let Ok(Some(reason)) = db.customer_suspend_reason(&key.brand_id, &input.email) {
        let message = if reason.is_empty() || reason == "--" {
            "Customer is already suspended by the admin.".to_string()
        } else {
            format!("Customer is already suspended by the admin. Reason: {reason}")
        };
        return api_error("INVALID_CUSTOMER", &message);
    }
    let now = now_utc();
    let payment_id = generate_item_id_len(27);
    let customer_info = json!({
        "name": input.full_name,
        "email": input.email,
        "mobile": input.mobile,
    })
    .to_string();
    let metadata = input.metadata.to_string();
    if db
        .create_transaction(
            &key.brand_id,
            &payment_id,
            &customer_info,
            input.amount_minor,
            &input.currency,
            &metadata,
            return_url.as_deref(),
            &webhook_url,
            &now,
        )
        .is_err()
    {
        return api_error(
            "INVALID_JSON_PAYLOAD",
            "The JSON payload is invalid or malformed.",
        );
    }
    if !db.customer_exists(&key.brand_id, &input.email).unwrap_or(false) {
        let ref_ = generate_item_id_len(10);
        let _ = db.create_customer(
            &ref_,
            &key.brand_id,
            &input.full_name,
            &input.email,
            &input.mobile,
            &now,
        );
    }
    let pp_url = format!("{}/payment/{}", site_url.trim_end_matches('/'), payment_id);
    (
        StatusCode::OK,
        Json(json!({"pp_id": payment_id, "pp_url": pp_url})),
    )
}

pub async fn checkout_redirect(
    headers: HeaderMap,
    body: Bytes,
    db: Arc<Db>,
    site_url: String,
) -> impl IntoResponse {
    let key = match auth(&headers, &db, "create_payment") {
        Ok(k) => k,
        Err(r) => return r,
    };
    let data = match parse_body(body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let return_url = match check_url(
        &db,
        data["return_url"].as_str().unwrap_or(""),
        "Return",
    ) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let webhook_url = match check_url(
        &db,
        data["webhook_url"].as_str().unwrap_or(""),
        "Webhook",
    ) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let input = match validate_checkout(&data) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let return_url = if return_url == "--" {
        None
    } else {
        Some(return_url)
    };
    finish_checkout(&db, &key, input, return_url, webhook_url, &site_url)
}

pub async fn checkout_popup(
    headers: HeaderMap,
    body: Bytes,
    db: Arc<Db>,
    site_url: String,
) -> impl IntoResponse {
    let key = match auth(&headers, &db, "create_payment") {
        Ok(k) => k,
        Err(r) => return r,
    };
    let data = match parse_body(body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let webhook_url = match check_url(
        &db,
        data["webhook_url"].as_str().unwrap_or(""),
        "Webhook",
    ) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let input = match validate_checkout(&data) {
        Ok(i) => i,
        Err(r) => return r,
    };
    finish_checkout(&db, &key, input, None, webhook_url, &site_url)
}

/// The bare transaction object shared by verify-payment and
/// refund-payment (PHP returns no wrapper around it). Also used for
/// the cron webhook payload.
pub(crate) fn transaction_view(db: &Db, row: &TransactionRow, status: &str) -> Value {
    let customer: Value =
        serde_json::from_str(&row.customer_info).unwrap_or(json!({}));
    let metadata: Value = serde_json::from_str(&row.metadata).unwrap_or(json!([]));
    let gateway = db
        .gateway_display(&row.brand_id, &row.gateway_id)
        .ok()
        .flatten()
        .unwrap_or_default();
    let total = row.amount + row.processing_fee - row.discount_amount;
    json!({
        "pp_id": row.ref_,
        "full_name": customer["name"].as_str().unwrap_or("N/A"),
        "email_address": customer["email"].as_str().unwrap_or("N/A"),
        "mobile_number": customer["mobile"].as_str().unwrap_or("N/A"),
        "gateway": gateway,
        "amount": money_to_string(row.amount),
        "fee": money_to_string(row.processing_fee),
        "discount_amount": money_to_string(row.discount_amount),
        "total": money_to_string(total),
        "local_net_amount": money_to_string(row.local_net_amount),
        "currency": row.currency,
        "local_currency": row.local_currency,
        "metadata": metadata,
        "sender": row.sender,
        "transaction_id": row.trx_id,
        "status": status,
        "date": format_user_time(&row.created_date),
    })
}

fn lookup_transaction(db: &Db, data: &Value) -> Result<TransactionRow, Reply> {
    let pp_id = data["pp_id"].as_str().unwrap_or("");
    if pp_id.is_empty() {
        return Err(api_error(
            "INVALID_PP_ID",
            "A valid bp id is required.",
        ));
    }
    match db.transaction_by_ref(pp_id).ok().flatten() {
        Some(row) => Ok(row),
        None => Err(api_error(
            "INVALID_PP_ID",
            "A valid bp id is required.",
        )),
    }
}

pub async fn verify_payment(
    headers: HeaderMap,
    body: Bytes,
    db: Arc<Db>,
) -> impl IntoResponse {
    let _key = match auth(&headers, &db, "verify_payment") {
        Ok(k) => k,
        Err(r) => return r,
    };
    let data = match parse_body(body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let row = match lookup_transaction(&db, &data) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let status = row.status.clone();
    (StatusCode::OK, Json(transaction_view(&db, &row, &status)))
}

pub async fn refund_payment(
    headers: HeaderMap,
    body: Bytes,
    db: Arc<Db>,
) -> impl IntoResponse {
    let _key = match auth(&headers, &db, "refund_payment") {
        Ok(k) => k,
        Err(r) => return r,
    };
    let data = match parse_body(body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let row = match lookup_transaction(&db, &data) {
        Ok(r) => r,
        Err(r) => return r,
    };
    if row.status != "completed" {
        return api_error(
            "INVALID_STATUS",
            "Only completed transactions can be refunded.",
        );
    }
    // bKash tokenized refunds need the API gateway, which is not in
    // Phase 1; PHP fails the same way when its config is missing.
    if db
        .gateway_slug(&row.brand_id, &row.gateway_id)
        .ok()
        .flatten()
        .as_deref()
        == Some("bkash-api-tokenized")
    {
        return api_error(
            "REFUND_FAILED",
            "bKash tokenized refund is not configured in piprapay-rs yet.",
        );
    }
    let now = now_utc();
    if db
        .update_transaction_status(row.id, "refunded", None, &now)
        .is_err()
    {
        return api_error(
            "INVALID_JSON_PAYLOAD",
            "The JSON payload is invalid or malformed.",
        );
    }
    (StatusCode::OK, Json(transaction_view(&db, &row, "refunded")))
}
