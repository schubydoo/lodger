-- Lodger's own data: accounts, sessions, recovery codes, the audit log, and UI
-- settings (TAD section 5.1). libvirt is the source of truth for VMs, pools,
-- and networks, so no table here holds VM data (PRD 5.6).
--
-- STRICT makes SQLite reject a value of the wrong type. Times are ISO 8601
-- strings in UTC.

CREATE TABLE accounts (
    id INTEGER PRIMARY KEY,
    -- Unique without regard to case: "Admin" and "admin" are one account.
    username TEXT NOT NULL UNIQUE COLLATE NOCASE,
    -- The argon2id PHC string.
    password_hash TEXT NOT NULL,
    -- The TOTP secret, encrypted with the key file in the state directory
    -- (TAD section 7.3). TOTP arrives in v1.1; the columns exist from the start.
    totp_secret_enc TEXT,
    totp_enabled INTEGER NOT NULL DEFAULT 0 CHECK (totp_enabled IN (0, 1)),
    created_at TEXT NOT NULL,
    password_changed_at TEXT NOT NULL
) STRICT;

CREATE TABLE sessions (
    -- Only the SHA-256 of the session token, never the token itself.
    token_sha256 BLOB PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    csrf_token TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    client_ip TEXT,
    user_agent TEXT
) STRICT;

CREATE INDEX sessions_account ON sessions (account_id);
CREATE INDEX sessions_expires ON sessions (expires_at);

CREATE TABLE recovery_codes (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    code_hash TEXT NOT NULL,
    used_at TEXT
) STRICT;

CREATE INDEX recovery_codes_account ON recovery_codes (account_id);

CREATE TABLE audit_log (
    id INTEGER PRIMARY KEY,
    ts TEXT NOT NULL,
    account_name TEXT,
    client_ip TEXT,
    event TEXT NOT NULL,
    -- The kind and name of the object an action touched, for example
    -- "vm" and "web1". A name, not a copy of the object.
    target_kind TEXT,
    target_name TEXT,
    job_id TEXT,
    result TEXT NOT NULL,
    -- Allowlisted fields only: never passwords, user-data, or tokens.
    detail_json TEXT
) STRICT;

CREATE INDEX audit_log_target ON audit_log (target_name, ts DESC);
-- For the daily deletion of rows older than 365 days (TAD section 5.2).
CREATE INDEX audit_log_ts ON audit_log (ts);

CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;
