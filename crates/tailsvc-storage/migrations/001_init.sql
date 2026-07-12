CREATE TABLE IF NOT EXISTS agents (
    agent_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    tailscale_ipv4 TEXT NOT NULL,
    docker_engine_id TEXT NOT NULL,
    credential_hash TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_seen_at TEXT,
    revoked_at TEXT
);

CREATE TABLE IF NOT EXISTS enrollment_tokens (
    token_hash TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    used_at TEXT,
    expires_at TEXT
);

CREATE TABLE IF NOT EXISTS routes (
    hostname TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(agent_id),
    backend TEXT NOT NULL,
    container_id TEXT,
    container_name TEXT,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_routes_agent ON routes(agent_id);

CREATE TABLE IF NOT EXISTS audit_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type TEXT NOT NULL,
    detail TEXT,
    created_at TEXT NOT NULL
);