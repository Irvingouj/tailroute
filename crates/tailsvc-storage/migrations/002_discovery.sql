-- Candidate discovery snapshots (last report per agent) and admin enable intents.

CREATE TABLE IF NOT EXISTS discovery_snapshots (
    agent_id TEXT PRIMARY KEY REFERENCES agents(agent_id) ON DELETE CASCADE,
    payload_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS enabled_services (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    identity_key TEXT NOT NULL,
    container_name TEXT,
    hostnames_json TEXT NOT NULL,
    backend TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (agent_id, identity_key)
);

CREATE INDEX IF NOT EXISTS idx_enabled_services_agent ON enabled_services(agent_id);
