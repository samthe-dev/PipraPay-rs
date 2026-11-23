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
- `CRON_SECRET` — secret for `GET /cron/<secret>` (falls back to the
  `cron-job` value in `pp_env` when set)
- `CRON_INTERVAL_SECS` — when set, also run settlement on an internal
  timer every N seconds; otherwise an external cron hitting
  `/cron/<secret>` drives it
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

## Cron settlement + webhooks

- `GET /cron/<secret>` — answers immediately, runs settlement in the
  background (10-minute run lock, like PHP's `cron.lock`).
- Settles `pending` transactions against approved SMS (sender_key +
  type + trx_id) within the brand's payment tolerance: SMS becomes
  `used`, the transaction becomes `completed`, and a webhook row is
  queued when the transaction has a webhook URL.
- Re-runs the longest-chain balance reconcile for every active
  balance verification row.
- Delivers pending webhooks (up to 15 per run, attempts limit from
  `pp_env` `geneal-application-settings-webhook_attempts_limit`,
  default 1): HTTP 200 = completed, otherwise pending, canceled once
  attempts reach the limit.
- Not in Phase 1: the automatic system update check and the currency
  auto-update fetch.

## Docker

```bash
docker build -t piprapay-rs .
docker run -p 8080:8080 -v piprapay-data:/data -e DATABASE_PATH=/data/piprapay.db piprapay-rs
```

### Docker Compose (recommended)

```bash
docker compose up --build -d
```

This uses `docker-compose.yml`: the app on port 8080, data in the named
volume `piprapay-data`, and the internal cron scheduler enabled
(`CRON_INTERVAL_SECS=60`). Override with environment variables, e.g.
`SITE_URL=https://pay.example.com CRON_SECRET=<secret> docker compose up -d`.
First run opens `http://localhost:8080/admin/setup` to create the first
admin and brand.

Notes:

- The image runs as the `nonroot` user (uid 65532). The named volume in
  the compose file is initialised with `/data` owned by that user, so it
  works out of the box. If you use a **bind mount** instead
  (`-v /host/path:/data`), chown the host directory first:
  `chown 65532:65532 /host/path`.
- Image size is ~33 MB (distroless + one static-ish binary); idle RSS
  is ~8 MB.
- No in-image `HEALTHCHECK` (distroless has no shell/curl): point your
  orchestrator at `GET /healthz`.

## Development

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```
