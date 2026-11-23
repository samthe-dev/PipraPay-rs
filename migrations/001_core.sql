-- 001_core.sql — PipraPay schema ported from pp-content/pp-install/db.sql
-- Money is INTEGER minor units (x10^8, matching decimal(20,8)), never float.
-- Dates are TEXT 'Y-m-d H:i:s' UTC. '--' is the empty sentinel, kept verbatim.
-- Enums are TEXT with CHECK constraints. No foreign keys in the PHP schema;
-- string-ID joins are enforced by the application, as in PHP.

CREATE TABLE IF NOT EXISTS pp_addon (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    addon_id TEXT NOT NULL DEFAULT '--',
    slug TEXT NOT NULL DEFAULT '--',
    name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','inactive'))
);
CREATE TABLE IF NOT EXISTS pp_addon_parameter (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    addon_id TEXT NOT NULL DEFAULT '--',
    option_name TEXT NOT NULL DEFAULT '--',
    value TEXT
);
CREATE TABLE IF NOT EXISTS pp_admin (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    a_id TEXT NOT NULL DEFAULT '--',
    full_name TEXT NOT NULL,
    username TEXT NOT NULL,
    email TEXT NOT NULL,
    password TEXT NOT NULL,
    temp_password TEXT,
    reset_limit TEXT NOT NULL DEFAULT '3',
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','suspend')),
    role TEXT NOT NULL DEFAULT 'admin' CHECK (role IN ('admin','staff')),
    twofa_status TEXT NOT NULL DEFAULT 'disable' CHECK (twofa_status IN ('enable','disable')),
    twofa_secret TEXT NOT NULL DEFAULT '--'
);
CREATE TABLE IF NOT EXISTS pp_api (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    brand_id TEXT NOT NULL DEFAULT '--',
    name TEXT NOT NULL,
    api_key TEXT NOT NULL,
    expired_date TEXT,
    api_scopes TEXT,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','inactive'))
);
CREATE INDEX IF NOT EXISTS idx_pp_api_key ON pp_api (brand_id, api_key);
CREATE TABLE IF NOT EXISTS pp_balance_verification (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id TEXT NOT NULL DEFAULT '--',
    sender_key TEXT NOT NULL DEFAULT '--',
    type TEXT NOT NULL DEFAULT 'Personal' CHECK (type IN ('Personal','Agent','Merchant')),
    current_balance INTEGER NOT NULL DEFAULT 0,
    simslot TEXT NOT NULL DEFAULT '--',
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','inactive'))
);
CREATE TABLE IF NOT EXISTS pp_brands (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    brand_id TEXT NOT NULL DEFAULT '--',
    favicon TEXT,
    logo TEXT,
    identify_name TEXT NOT NULL DEFAULT 'Default',
    name TEXT NOT NULL,
    email TEXT,
    phone TEXT,
    website TEXT,
    whatsapp TEXT,
    telegram TEXT,
    messenger TEXT,
    facebook_page TEXT,
    theme TEXT NOT NULL DEFAULT 'twenty-six',
    address TEXT,
    timezone TEXT NOT NULL DEFAULT 'Asia/Dhaka',
    language TEXT NOT NULL DEFAULT 'en',
    currency_code TEXT NOT NULL DEFAULT 'BDT',
    auto_exchange TEXT NOT NULL DEFAULT 'disabled' CHECK (auto_exchange IN ('disabled','enabled')),
    payment_tolerance TEXT NOT NULL DEFAULT '0'
);
CREATE TABLE IF NOT EXISTS pp_browser_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    a_id TEXT NOT NULL DEFAULT '--',
    cookie TEXT NOT NULL,
    browser TEXT,
    device TEXT,
    ip TEXT,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','expired'))
);
CREATE TABLE IF NOT EXISTS pp_currency (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    brand_id TEXT NOT NULL DEFAULT '--',
    code TEXT NOT NULL,
    symbol TEXT,
    rate INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS pp_customer (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ref TEXT NOT NULL DEFAULT '--',
    brand_id TEXT NOT NULL DEFAULT '--',
    name TEXT,
    email TEXT,
    mobile TEXT,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','suspend')),
    suspend_reason TEXT,
    inserted_via TEXT NOT NULL DEFAULT 'manual' CHECK (inserted_via IN ('manual','checkout'))
);
CREATE TABLE IF NOT EXISTS pp_device (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    d_id TEXT NOT NULL DEFAULT '--',
    device_id TEXT NOT NULL DEFAULT '--',
    otp TEXT NOT NULL DEFAULT '--',
    name TEXT,
    model TEXT,
    android_level TEXT,
    app_version TEXT,
    status TEXT NOT NULL DEFAULT 'processing' CHECK (status IN ('processing','used')),
    last_sync TEXT NOT NULL DEFAULT '--'
);
CREATE INDEX IF NOT EXISTS idx_pp_device_otp ON pp_device (otp);
CREATE INDEX IF NOT EXISTS idx_pp_device_id ON pp_device (device_id);
CREATE TABLE IF NOT EXISTS pp_domain (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    domain TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','inactive'))
);
CREATE TABLE IF NOT EXISTS pp_env (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    brand_id TEXT NOT NULL DEFAULT 'both',
    option_name TEXT NOT NULL,
    value TEXT
);
CREATE INDEX IF NOT EXISTS idx_pp_env ON pp_env (brand_id, option_name);
CREATE TABLE IF NOT EXISTS pp_faq (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    brand_id TEXT NOT NULL DEFAULT '--',
    title TEXT,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','inactive'))
);
CREATE TABLE IF NOT EXISTS pp_gateways (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    gateway_id TEXT NOT NULL DEFAULT '--',
    brand_id TEXT NOT NULL DEFAULT '--',
    slug TEXT NOT NULL DEFAULT '--',
    name TEXT,
    display TEXT,
    logo TEXT,
    currency TEXT,
    min_allow INTEGER NOT NULL DEFAULT 0,
    max_allow INTEGER NOT NULL DEFAULT 0,
    fixed_discount INTEGER NOT NULL DEFAULT 0,
    percentage_discount INTEGER NOT NULL DEFAULT 0,
    fixed_charge INTEGER NOT NULL DEFAULT 0,
    percentage_charge INTEGER NOT NULL DEFAULT 0,
    primary_color TEXT,
    text_color TEXT,
    btn_color TEXT,
    btn_text_color TEXT,
    tab TEXT NOT NULL DEFAULT 'mfs' CHECK (tab IN ('mfs','bank','global')),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','inactive'))
);
CREATE TABLE IF NOT EXISTS pp_gateways_parameter (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    brand_id TEXT NOT NULL DEFAULT '--',
    gateway_id TEXT NOT NULL DEFAULT '--',
    option_name TEXT NOT NULL,
    value TEXT
);
CREATE TABLE IF NOT EXISTS pp_invoice (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ref TEXT NOT NULL DEFAULT '--',
    brand_id TEXT NOT NULL DEFAULT '--',
    customer_info TEXT,
    gateway_id TEXT NOT NULL DEFAULT '--',
    currency TEXT,
    due_date TEXT,
    shipping TEXT NOT NULL DEFAULT '0',
    status TEXT CHECK (status IN ('paid','unpaid','refunded','canceled'))
);
CREATE TABLE IF NOT EXISTS pp_invoice_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    brand_id TEXT NOT NULL DEFAULT '--',
    invoice_id TEXT NOT NULL DEFAULT '--',
    description TEXT,
    amount INTEGER NOT NULL DEFAULT 0,
    discount INTEGER NOT NULL DEFAULT 0,
    vat INTEGER NOT NULL DEFAULT 0,
    quantity INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS pp_payment_link (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ref TEXT NOT NULL DEFAULT '--',
    brand_id TEXT NOT NULL DEFAULT '--',
    product_info TEXT,
    amount INTEGER NOT NULL DEFAULT 0,
    quantity INTEGER NOT NULL DEFAULT 0,
    currency TEXT,
    expired_date TEXT,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','inactive'))
);
CREATE TABLE IF NOT EXISTS pp_payment_link_field (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    payment_link_id TEXT NOT NULL DEFAULT '--',
    form_type TEXT,
    field_name TEXT,
    value TEXT,
    required TEXT NOT NULL DEFAULT 'true' CHECK (required IN ('true','false'))
);
CREATE TABLE IF NOT EXISTS pp_permission (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    brand_id TEXT NOT NULL DEFAULT '--',
    a_id TEXT NOT NULL DEFAULT '--',
    permission TEXT,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','suspend'))
);
CREATE TABLE IF NOT EXISTS pp_sms_data (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT NOT NULL DEFAULT 'web' CHECK (source IN ('app','web')),
    device_id TEXT NOT NULL DEFAULT '--',
    sender TEXT NOT NULL DEFAULT '--',
    sender_key TEXT NOT NULL DEFAULT '--',
    simslot TEXT,
    number TEXT NOT NULL DEFAULT '--',
    amount INTEGER NOT NULL DEFAULT 0,
    currency TEXT NOT NULL DEFAULT '--',
    trx_id TEXT NOT NULL DEFAULT '--',
    balance TEXT NOT NULL DEFAULT '--',
    message TEXT,
    reason TEXT,
    type TEXT NOT NULL DEFAULT 'Personal' CHECK (type IN ('Personal','Agent','Merchant')),
    entry_type TEXT NOT NULL DEFAULT 'automatic' CHECK (entry_type IN ('manual','automatic')),
    edit_status TEXT NOT NULL DEFAULT 'pending' CHECK (edit_status IN ('done','pending')),
    status TEXT NOT NULL DEFAULT 'approved' CHECK (status IN ('approved','awaiting-review','used','error'))
);
CREATE INDEX IF NOT EXISTS idx_pp_sms_trx ON pp_sms_data (sender_key, trx_id);
CREATE TABLE IF NOT EXISTS pp_transaction (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    brand_id TEXT NOT NULL DEFAULT '--',
    source TEXT NOT NULL DEFAULT 'api' CHECK (source IN ('invoice','payment-link','payment-link-default','api')),
    ref TEXT NOT NULL DEFAULT '--',
    customer_info TEXT,
    amount INTEGER NOT NULL DEFAULT 0,
    processing_fee INTEGER NOT NULL DEFAULT 0,
    discount_amount INTEGER NOT NULL DEFAULT 0,
    local_net_amount INTEGER NOT NULL DEFAULT 0,
    currency TEXT,
    local_currency TEXT,
    sender TEXT NOT NULL DEFAULT '--',
    trx_id TEXT NOT NULL DEFAULT '--',
    trx_slip TEXT,
    gateway_id TEXT NOT NULL DEFAULT '--',
    sender_key TEXT NOT NULL DEFAULT '--',
    sender_type TEXT,
    source_info TEXT,
    metadata TEXT,
    status TEXT NOT NULL DEFAULT 'initiated' CHECK (status IN ('completed','pending','refunded','initiated','canceled')),
    return_url TEXT,
    webhook_url TEXT
);
CREATE INDEX IF NOT EXISTS idx_pp_transaction_ref ON pp_transaction (brand_id, ref, trx_id);
CREATE TABLE IF NOT EXISTS pp_webhook_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ref TEXT NOT NULL DEFAULT '--',
    brand_id TEXT NOT NULL DEFAULT '--',
    payload TEXT,
    url TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    response_body TEXT,
    http_code TEXT,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('completed','pending','canceled'))
);
