//! Companion (Android) handlers and SMS parsing.
//!
//! The Android app sends raw SMS verbatim; all parsing is server-side.
//! Regexes are ported verbatim from PHP `MFSMessageVerified()` in
//! `pp-functions.php:889-1168`, in priority order, first match wins.
//! This file currently holds the dispatch stubs and the parser skeleton;
//! the full regex table lands with the SMS engine implementation.

use serde_json::{Value, json};
use std::collections::HashMap;

/// Whitelist senders, exact-match lowercased, from PHP `senderWhitelist()`.
/// Note the verbatim oddities: `tap.` has a trailing dot, CellFin's sender
/// is literally `ibbl .` (space + dot).
pub const SENDERS: &[&str] = &[
    "bkash",
    "nagad",
    "16216",
    "upay",
    "tap.",
    "ibbl .",
    "01847-348685",
    "16259",
    "pathaopay",
    "telecash",
    "09638-900800",
];

pub fn companion_login(_form: &HashMap<String, String>) -> Value {
    // TODO: look up pp_device by otp, rotate otp, set status="used".
    json!({"status": "false", "title": "Not implemented", "message": "login is not implemented yet"})
}

pub fn companion_account_information(_form: &HashMap<String, String>) -> Value {
    // TODO: resolve device -> admin, return counts and stored/used/error lists.
    json!({"status": "false", "title": "Not implemented", "message": "account-information is not implemented yet"})
}

pub fn companion_senders(_form: &HashMap<String, String>) -> Value {
    json!({"status": "true", "senders": SENDERS})
}

pub fn companion_bulk(_form: &HashMap<String, String>) -> Value {
    // TODO: parse sms_list, verify each SMS, store in pp_sms_data.
    // Return ONE JSON object for the whole bulk (fixes the PHP
    // concatenated-JSON bug), with per-item results inside.
    json!({"status": "false", "title": "Not implemented", "message": "sms-transmit-bulk is not implemented yet"})
}

pub fn companion_delete(_form: &HashMap<String, String>) -> Value {
    json!({"status": "false", "title": "Not implemented", "message": "delete-sms-data is not implemented yet"})
}
