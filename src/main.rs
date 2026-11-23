mod db;
mod sms;

use axum::{
    Form, Json, Router,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
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

/// Companion API: POST / with form field `action-companion`.
/// This is the Android app's only server surface (see STUDY.md section 2).
/// Handlers are implemented incrementally; unknown actions return the
/// PHP-shaped string status "false".
async fn companion(
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let action = form.get("action-companion").cloned().unwrap_or_default();
    let reply = match action.as_str() {
        "login" => sms::companion_login(&form),
        "account-information" => sms::companion_account_information(&form),
        "sms-transmit-sender" => sms::companion_senders(&form),
        "sms-transmit-bulk" => sms::companion_bulk(&form),
        "delete-sms-data" => sms::companion_delete(&form),
        "" => json!({"status": "false", "title": "Missing action", "message": "action-companion is required"}),
        _ => json!({"status": "false", "title": "Unknown action", "message": "unknown action-companion"}),
    };
    (StatusCode::OK, Json(reply))
}

#[derive(Deserialize)]
struct _Unused;
