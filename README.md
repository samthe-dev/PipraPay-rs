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
- `SITE_URL` (default `http://localhost:8080`) — base URL used in checkout links
- `RUST_LOG` (default `piprapay_rs=info`)

## Endpoints (Phase 1, in progress)

- `GET /healthz` — `{"status":"ok"}`, for Docker healthcheck.
- `POST /` — Android companion API, form-encoded, `action-companion`
  is `login`, `account-information`, `sms-transmit-sender`,
  `sms-transmit-bulk`, or `delete-sms-data`. All five are implemented.
- `POST /api/checkout/redirect` and `POST /api/checkout/popup` —
  merchant API, JSON body, header `MHS-PIPRAPAY-API-KEY`, scope
  `create_payment`. Returns `{"pp_id","pp_url"}`.
- `POST /api/verify-payment` — scope `verify_payment`. Returns the
  bare transaction object for a `pp_id`.
- `POST /api/refund-payment` — scope `refund_payment`. Only
  `completed` transactions can be refunded. bKash tokenized refunds
  are not configured yet (API gateways are not in Phase 1) and return
  `REFUND_FAILED`.
- Errors are HTTP 400 with `{"error":{"code","message"}}`, matching PHP.

## Admin panel

- `GET /admin/setup` — first-run setup when no admin exists: creates
  the first admin + brand + BDT currency + full permission.
- `GET /admin/login` — username/email + bcrypt password, TOTP 2FA when
  enabled (`/admin/2fa`), session cookie `pp_admin` in `pp_browser_log`.
- `GET /admin/dashboard` — transaction/SMS counts, recent transactions.
- `GET /admin/devices` — list + connect (generates the OTP and the
  `{baseUrl}----{otp}` QR text the Android app scans).
- `GET /admin/sms` — SMS data with status filter, approve awaiting-review.
- `GET /admin/transactions`, `GET /admin/brands`, `GET /admin/gateways`.
- All admin POSTs require the per-session CSRF token.

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
