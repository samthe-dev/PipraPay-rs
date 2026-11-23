# piprapay-rs

PipraPay in Rust: a low-RAM/CPU self-hosted payment automation server.
Rust + SQLite (WAL mode), one static binary, one Docker container.
No PHP, no MariaDB, no Apache.

See [STUDY.md](STUDY.md) for the full study of the PHP server and the
Android app, the compatibility contract, and the tech stack discussion.
Phase 1 includes the admin panel (decided 2026-11-24).

## Run locally

```bash
cargo run
# listens on 0.0.0.0:8080, SQLite file ./piprapay.db
```

Environment:

- `DATABASE_PATH` (default `piprapay.db`)
- `LISTEN_ADDR` (default `0.0.0.0:8080`)
- `RUST_LOG` (default `piprapay_rs=info`)

## Endpoints (Phase 1, in progress)

- `GET /healthz` — `{"status":"ok"}`, for Docker healthcheck.
- `POST /` — Android companion API, form-encoded, `action-companion`
  is `login`, `account-information`, `sms-transmit-sender`,
  `sms-transmit-bulk`, or `delete-sms-data`. Handlers are being
  implemented incrementally; unimplemented actions return
  `{"status":"false","message":"... not implemented yet"}`.

## Docker

```bash
docker build -t piprapay-rs .
docker run -p 8080:8080 -v piprapay-data:/data -e DATABASE_PATH=/data/piprapay.db piprapay-rs
```

## Development

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```
