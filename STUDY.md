# PipraPay Rust Rewrite — Study and Tech Stack Discussion

Date: 2026-11-24
Status: Discussion only, no code written yet.
Sources: `../PipraPay/` (Seizmann/PipraPay, 449 files, PHP 8.1-8.3 + MySQL 8.0, AGPL-3.0)
and `../PipraPay-V3-App/` (samsusiyam/PipraPay-V3-App v3.0.2, Java, Android).
Method: source reading. REA was checked first: REA is for shipped binaries /
decompilation, and its skill says to skip REA for ordinary source-repo analysis.
`rea inspect-artifact` on both repos refused them as source directories, not
binaries, which confirms there is nothing to decompile — the rewrite is from source.

## 1. What PipraPay actually is

Vanilla PHP, no framework (README badge says Laravel 12.x, there is no
composer.json and no framework in the repo). Single entry `index.php`
(2,109 lines), flat `switch` router, `pp-adapter.php` (9,677 lines) as a
second POST dispatcher that runs on every request and `exit()`s for POSTs,
`pp-functions.php` (3,540 lines) with helpers and a PDO QueryBuilder.
DB-backed key/value settings (`pp_env`), plugin gateways as PHP classes.

Core engine: SMS-based payment verification. No API from bKash/Nagad/Rocket
in the normal flow — the Android phone receives the payment SMS, forwards the
raw SMS to the server, the server parses TrxID / amount / sender with regex,
matches it against a pending transaction, and marks it completed.

## 2. The contract the Rust server MUST keep (Android unchanged)

The Android app (v3.0.2, `com.qube.piprapay_tool`) calls exactly one route:

- `POST /` (site root), form-encoded (`application/x-www-form-urlencoded`),
  no headers, no Bearer token, distinguished only by `action-companion`.
- `login`: `onetimepassword, name, model, android_level, app_version="3.0.2"`.
  Success returns JSON with `status: "true"` (string, not boolean) and/or
  `token`. Auth is `pp_device.otp`: login looks up `otp` with no status
  filter, then rotates it to a new 10-char ID and sets `status="used"`.
  Later calls require `otp = token AND status="used"`.
  QR format is literally `{baseUrl}----{otp}`.
- `account-information`: `token`. Returns `fullname, email, stored_count,
  used_count, error_count, stored[], used[], error[]`, each item with
  `sender, simslot, message, timestamp, reason` (error only).
  This is also the app's "Ping" — there is no ping endpoint.
- `sms-transmit-sender`: `token`. Returns `{"senders": [...]}` — the
  whitelist the app filters on. Server list is exact-match, lowercased.
- `sms-transmit-bulk`: `token, sms_list` (a JSON-encoded string of
  `[{id, sender, message, simSlot, timestamp}]`, timestamp in seconds as a
  string). The app does NO parsing; the server must parse the raw message.
- `delete-sms-data`: exists server-side, never called by the app.

No logout, heartbeat, device-status, or update endpoint. The app's updater
hits GitHub releases directly, not the PipraPay server.

### Bugs / quirks to decide on, not silently fix

1. Bulk returns concatenated JSON: PHP echoes one JSON object per SMS in a
   loop, with no `exit()` except on Code 102. Sending >1 SMS returns
   `{...}{...}`, which the app cannot parse as one JSONObject, so it
   re-queues and retries. Single-SMS calls work.
   Proposal: fix this in Rust (return ONE JSON object for the whole bulk,
   with per-item results). The app treats any parseable JSON as success,
   so a single valid object is strictly better and still compatible.
2. Dedup is only `(sender_key, trx_id)` for parsed messages. Unparseable
   attempts insert an `error` row on every retry. The Android `id` field
   is ignored server-side and is not stable (rebuilt on retry).
   Proposal: dedup by parsed TrxID + message hash, idempotent bulk.
3. Whitelist mismatch: server exact-match senders are `bkash, nagad,
   16216, upay, tap., ibbl . , 01847-348685, 16259, pathaopay, telecash,
   09638-900800`. Android defaults are substring-match
   `bkash, nagad, 16216, 16167, rocket, upay, cellfin, ibbl`.
   `16167` / `cellfin` / `ibbl` are not server senders; CellFin SMS
   arrives as `sender_key='--'` and fails parsing (Code 101).
   Proposal: keep the server's exact list as canonical, keep Android's
   substring behaviour unchanged (it is the app's code).
4. `account-information` SQL excludes `awaiting-review`, so that branch
   is dead code; the dashboard never sees awaiting-review items.
5. Device auth is coupled to an admin session: `account-information`
   needs a `pp_browser_log` row linking `device.d_id` to an admin.
   If that row is gone, a valid device token fails with no recovery
   except re-pairing. Proposal: decouple in Rust (device token alone
   is enough for companion calls).

## 3. SMS parsing — the single source of truth

`MFSMessageVerified()` in `pp-functions.php:889-1168`, plus
`senderWhitelist()` at `:768-887`. Whitespace is normalised, formats are
sorted by priority DESC, first regex match wins, commas are stripped
from amount / balance / fee.

Providers with a sender entry: bkash, nagad, rocket, upay, tap, cellfin,
okwallet, mcash, pathaopay, telecash, ipay.
Providers with an actual parse regex: only bkash, nagad, rocket, upay,
tap, cellfin, okwallet, mcash, pathaopay (9). telecash and ipay have NO
format — their SMS can never parse. Only Personal patterns are active,
plus one bKash Merchant pattern; all other Merchant/Agent patterns are
commented out. SureCash / Easypaisa / JazzCash have no code at all,
only README marketing text.

Active regexes (verbatim, port them exactly, in priority order):
- bkash Personal p100 / p90, bkash Merchant p80
- nagad Personal p100 / p90
- rocket Personal p100, upay Personal p100, tap Personal p100
- cellfin Personal p100 (no balance in the message)
- okwallet Personal p100, mcash Personal p100, pathaopay Personal p100

Balance verification: only if `balance_verify='true'` and an active
`pp_balance_verification` row exists for device + sender_key + type.
`expected = current_balance + amount` must equal the SMS balance, and
the SIM slot must match. Match → `approved`, balance advanced.
Mismatch → `awaiting-review`, then `reconcileByLongestChain()` builds
chains where each SMS's `prev = balance - amount` links to the previous
balance, starting from the canonical balance; the longest chain is
approved. cellfin has `balance_verify='false'`, so it is approved
unconditionally.

Verification / settlement:
- Checkout TrxID verify (`action-v2 transaction-verify`): duplicate
  TrxID check, look up approved `pp_sms_data` by
  `(sender_key, type, trx_id)`, amount check
  `paid >= checkout AND paid <= checkout + brand.payment_tolerance`
  (bcmath, 8 dp). Pass → SMS `used`, transaction `completed`.
- Cron settlement: same match for pending transactions.
- Manual verify: TrxID duplicate check only → `pending`; slip upload
  (5 MB cap) → `pending`, admin approves.
- Transaction lifecycle: `initiated → pending → completed`
  (+ `refunded` / `canceled`). SMS lifecycle:
  `approved → used` / `awaiting-review` / `error`.

## 4. Merchant API (public, `?page=api/...`)

Auth: header `MHS-PIPRAPAY-API-KEY` only, no Bearer, no query key.
Body: raw JSON from `php://input`. Errors: HTTP 400
`{error:{code, message}}`. HTTP method is not checked in the code.

- `api/checkout/redirect` — scope `create_payment`. Needs
  `full_name, email_address, mobile_number, amount, currency` (default
  BDT), `return_url, webhook_url, metadata`. Returns
  `{pp_id, pp_url}`. `return_url` / `webhook_url` domains must exist
  in `pp_domain` and be active.
- `api/checkout/popup` — same minus `return_url`; webhook domain
  whitelist is NOT checked here (gap in PHP, preserve or fix?).
- `api/verify-payment` — scope `verify_payment`, `{pp_id}`.
  Returns the BARE transaction object, no status wrapper.
- `api/refund-payment` — scope `refund_payment`, `{pp_id}`.
  Only `status="completed"` is refundable. Only `bkash-api-tokenized`
  actually calls a gateway refund; every other gateway is just
  marked `refunded` locally.

## 5. Gateways — 48 directories, 3 kinds

- Automation (30): SMS-verified only, no `process_payment` / `callback` /
  `ipn`. All BDT, `tab='mfs'`. Slug → class name rule:
  `bkash-personal` → `BkashPersonalGateway`. Admin edit injects
  `mobile_number` + `pending_payment` for every automation gateway.
  Sender keys: bkash, nagad, rocket, upay, tap, cellfin, mcash,
  ipay, okwallet, pathaopay, telecash, each with Personal / Merchant /
  Agent variants (cellfin has no Agent, pathaopay has no Agent).
- API (13): aamarpay, binance-personal, bkash-api-tokenized, eps,
  nagad-merchant-api, nowpayments, oxapay, pathaopay-merchant-api,
  paypal-api, paystation, shurjopay, sslcommerz, stripe. Each has
  `process_payment` + usually `callback` and/or `ipn`, with credentials
  in `pp_gateways_parameter` (plaintext in PHP).
- Manual (5, USD, `tab='global'`): payeer, payoneer, paypal, taptap-send,
  wise. Verification is customer-submitted TrxID only → `pending`.
  Bank gateways (`tab='bank'`, no class file) behave as manual with
  slip verification.

One theme shipped: `twenty-six`. No addon classes shipped.

## 6. Database — 23 tables, `pp-content/pp-install/db.sql`

No foreign keys, no unique constraints, no seed rows in the dump.
Dates are `varchar(20)` holding `Y-m-d H:i:s` (UTC). Money is
`decimal(20,8)`. The sentinel `'--'` means empty/null in almost every
varchar/text column. IDs are short strings, `generateItemID()` 10 chars.

Tables: pp_addon, pp_addon_parameter, pp_admin, pp_api,
pp_balance_verification, pp_brands, pp_browser_log, pp_currency,
pp_customer, pp_device, pp_domain, pp_env, pp_faq, pp_gateways,
pp_gateways_parameter, pp_invoice, pp_invoice_items, pp_payment_link,
pp_payment_link_field, pp_permission, pp_sms_data, pp_transaction,
pp_webhook_log.

Key tables for the Rust core: `pp_device` (d_id, device_id, otp,
status processing/used, last_sync), `pp_sms_data` (source app/web,
device_id, sender, sender_key, simslot, number, amount, currency,
trx_id, balance, message, reason, type Personal/Agent/Merchant,
entry_type, edit_status, status approved/awaiting-review/used/error),
`pp_transaction` (brand_id, source, ref, customer_info JSON, amount,
fees, currency, sender, trx_id, trx_slip, gateway_id, sender_key,
status initiated/pending/completed/refunded/canceled, return/webhook
URLs), `pp_api` (api_key, scopes, expired_date, status),
`pp_env` (brand_id='both' = global, option_name, value — settings,
cron key, etc.), `pp_webhook_log` (payload, url, attempts, http_code,
status pending/completed/canceled), `pp_brands`, `pp_currency`,
`pp_gateways`, `pp_gateways_parameter`.

## 7. Cron and webhooks

`?page=cron/<secret>`, secret is `pp_env` `cron-job`. Responds
`{status:'true', message:"Cron run executed."}` BEFORE doing the work.
10-minute lock file `pp-media/storage/cron.lock`. Jobs in order:
1. auto-update check against `updates.piprapay.com/manifest.json`
   (every 10 h, if enabled),
2. verify pending transactions against approved SMS,
3. auto currency refresh via `cdn.jsdelivr.net/.../currency-api`
   (every 5 h per brand with autoExchange enabled),
4. balance verification via `reconcileByLongestChain()`,
5. flush pending `pp_webhook_log` (limit from settings, default 1
   attempt; 15 rows at a time) via parallel curl_multi, 3 s connect /
   5 s timeout; HTTP 200 → completed, else pending, attempts >= limit
   + non-200 → canceled.

Outbound webhooks: POST JSON, no signature header. Payload is a
completed transaction. No expiry/cleanup job exists; expiry is only
checked inline at request time.

## 8. Admin panel

`pp-admin/index.php` shell, login / 2fa / forgot pages, `pp-root/`
pages for dashboard, transactions, invoices, payment links, gateways,
sms-data, devices, brands, staff, domains, addons, system settings.
89 distinct `$_POST['action']` values. Auth: `pp_admin` cookie →
`pp_browser_log` (status active) → admin + permission JSON
(`pp_permission.permission`); `role='admin'` bypasses all checks.
Passwords are bcrypt, 2FA is TOTP, CSRF is a session token.
Admin is NOT used by Android.

## 9. Proposed tech stack for piprapay-rs

Goal: replace PHP 8.2 + Apache + MySQL 8.0 (hundreds of MB RAM, a
separate DB process) with one small static Rust binary + one SQLite
file, in one Docker container, idle RSS target under 50 MB.

### Proposed (recommended)

- Language: Rust, stable, edition 2024.
- Web framework: Axum 0.8 on Tokio. Single binary, no framework
  server process, no PHP-FPM. Axum is the lightest mainstream choice
  that still gives routing, extractors, and middleware without a
  heavy runtime. Actix-web is faster on benchmarks but heavier and
  more complex; raw Hyper would save a few MB but costs a lot of
  hand-written routing. Use Axum.
- Async runtime: Tokio, multi-thread with 2 worker threads
  (`TOKIO_WORKER_THREADS=2`), or `current_thread` if the host is
  truly tiny (1 vCPU / 256 MB). Keep it configurable by env.
- HTTP server: Axum / Hyper, no Apache/Nginx in front by default.
  Optional reverse proxy in front is the deployer's choice, not
  part of the container.
- Database: SQLite, WAL mode, via `rusqlite` with the bundled SQLite
  (no separate DB process, no network). `sqlx` is the alternative if
  we want async + migrations, but `rusqlite` + a small blocking pool
  is simpler, smaller, and enough for this workload (SMS ingest is
  low-rate, mostly single-writer). WAL pragmas:
  `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`,
  `busy_timeout=5000`, `cache_size=-2000` (2 MB), `temp_store=MEMORY`.
  One writer, a small read pool (2-4 connections). Money as `i64`
  minor units (amount x 10^8, matching PHP `decimal(20,8)`) — never
  float. Dates as TEXT `Y-m-d H:i:s` UTC to keep PHP compatibility,
  or ISO 8601 — decision needed, see below.
- Migrations: `refinery` or plain SQL files run at startup, versioned.
  Keep the 23 table names and columns for data compatibility, add
  SQLite indexes for the hot lookups (`pp_transaction(ref)`,
  `pp_sms_data(sender_key, trx_id)`, `pp_device(otp)`).
  Enums become TEXT with CHECK constraints. `'--'` sentinel:
  keep it verbatim in the DB for compatibility, normalise to
  `None` only at the Rust boundary — decision needed.
- SMS parsing: `regex` crate, port the PHP regexes verbatim in
  priority order. No new parser design in v1.
- Passwords: `bcrypt` crate to verify existing PHP hashes;
  new passwords with Argon2id (`argon2` crate) or keep bcrypt for
  simplicity — decision needed. TOTP: `totp-lite`.
- Sessions / CSRF: same shape as PHP — random cookie in
  `pp_browser_log`, HttpOnly, SameSite=Lax; CSRF token per session.
  Do NOT reproduce the hardcoded HMAC secret found in
  `pp-adapter.php` for the `pp-app-token` path, and do NOT reproduce
  the no-op `escape_string()` — use bound parameters everywhere.
  Gateway credentials: store encrypted at rest in v1? PHP stores
  them plaintext in `pp_gateways_parameter`. Proposal: keep the
  column, encrypt values with a key from env (age / AES-GCM) in a
  later step, do not block v1 on it — decision needed.
- HTTP client (webhooks, currency API, bKash/Stripe in later phases):
  `reqwest` with `rustls` (no OpenSSL in the image, smaller and
  static). Timeouts 3 s connect / 5 s total, matching PHP.
- Templates / admin UI: defer. Phase 1 is API-only (companion +
  merchant API + cron + SMS engine). Admin panel is the biggest
  surface (89 actions) and should be Phase 2, using Askama
  (compile-time templates, zero runtime cost) + a small amount of
  HTMX, or a minimal JSON admin API + a tiny static UI.
  Reusing the PHP Tabler theme HTML is possible but is a large port.
- Background jobs: keep the cron URL contract
  (`/cron/<secret>`, respond first, 10-min lock) for compatibility,
  implemented with a DB/file lock in Rust. Also add an internal
  Tokio interval task (every 60 s) so the container works with no
  external cron at all — the external cron URL stays as a manual
  trigger. Do not run both at once without the lock.
- Config: env vars only, no PHP config file. `DATABASE_PATH`
  (default `/data/piprapay.db`), `LISTEN_ADDR` (default
  `0.0.0.0:8080`), `RUST_LOG`, secrets from env / Docker secrets.
  First-run setup: a small CLI / init step that creates the first
  admin + brand + BDT currency, mirroring `pp-install/index.php`.
- Logging: `tracing` + `tracing-subscriber`, JSON or plain text,
  minimal fields. No metrics server in v1; add a tiny `/healthz`
  endpoint (not part of the PHP contract, for Docker healthcheck).
- Docker: multi-stage build with `cargo-chef` for caching.
  Builder: `rust:1.x-bookworm` (or alpine). Final image:
  `scratch` or `gcr.io/distroless/cc` with the static binary +
  `ca-certificates` only. No shell, no package manager in the
  final image. Volume `/data` for the SQLite file (+ WAL).
  Expected image ~15-25 MB, idle RSS ~15-30 MB, vs PHP + Apache +
  MySQL which is typically 400-800 MB idle.
- Build: `cargo build --release`, `strip`, `lto = true`,
  `codegen-units = 1`, `panic = "abort"` for the release profile
  to keep the binary small. Keep debug symbols out of the image.

### Alternatives considered

- Actix-web: faster, but a bigger binary and a more complex
  actor model for this small service. Not recommended.
- sqlx + async SQLite: nicer migrations and async, but a larger
  dependency tree and async SQLite still serialises writes.
  Revisit if the admin panel needs many concurrent writers.
- PostgreSQL: defeats the goal — a second process and far more RAM.
  Not recommended. SQLite WAL is enough for a single-node
  self-hosted payment relay; if a deployment outgrows it, that is
  a deliberate v2 decision, not a v1 default.
- Go: would also be small, but the user asked for Rust and the
  team already works in Rust.

### Proposed phases

- Phase 1 (core, Android-compatible): companion API (all 5 actions,
  with the bulk-JSON fix), merchant API (checkout redirect/popup,
  verify, refund-local), SMS ingest + parsing + balance chain +
  cron settlement, webhook queue, SQLite schema + migrations,
  Docker image, `/healthz`. No API gateways.
  Admin bootstrap via CLI/env only.
- Phase 2: admin panel (Askama + HTMX) and the 89 admin actions,
  starting with devices, SMS data, transactions, brands, gateways.
- Phase 3: API gateways (bKash tokenized, Stripe, SSLCommerz, etc.)
  one at a time, each behind a feature flag, with credentials
  encrypted at rest.

### Decision recorded 2026-11-24

- Sijan chose: Phase 1 WITH the admin panel (not API-only).
  So the admin panel moves from Phase 2 into Phase 1.
  Admin UI approach stays Askama (compile-time templates) + HTMX,
  server-rendered, starting with devices, SMS data, transactions,
  brands, gateways, then the remaining admin actions.
  This makes Phase 1 larger than the original API-only proposal.
  No other open decision below has been decided yet.

## 10. Open decisions for Sijan

1. Scope of Phase 1: companion + merchant API only, or include
   the admin panel from the start?
2. Bulk SMS JSON: fix the concatenated-JSON bug (one JSON object
   per bulk call, per-item results) or reproduce it byte-for-byte?
   Recommended: fix it.
3. Money: `i64` minor units (x10^8) internally, TEXT/decimal
   shape at the API boundary — agreed?
4. Dates: keep PHP `Y-m-d H:i:s` TEXT (UTC) for compatibility,
   or move to ISO 8601 / Unix seconds in Rust?
5. `'--'` sentinel: keep verbatim in SQLite, or normalise to NULL
   and translate at the boundary?
6. Passwords: keep bcrypt for compatibility, or Argon2id for new
   passwords with bcrypt verification for migrated hashes?
7. API gateways in Phase 1: none (recommended), or bKash tokenized
   only?
8. Data migration: fresh install only in v1, or a MySQL → SQLite
   import tool for existing PipraPay installs?
9. Admin UI in Phase 2: Askama + HTMX server-rendered, or a
   JSON API + separate frontend?
10. Cron: keep the external cron URL only, or also run an
    internal 60 s scheduler (recommended, with the lock)?
