CREATE TABLE integration_jobs (
    id TEXT PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE,
    request TEXT NOT NULL CHECK(json_valid(request)),
    connection_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    payload TEXT NOT NULL CHECK(json_valid(payload)),
    plan TEXT NOT NULL CHECK(json_valid(plan)),
    actor TEXT NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id),
    required_scope TEXT NOT NULL,
    safe_retry INTEGER NOT NULL CHECK(safe_retry IN(0,1)),
    status TEXT NOT NULL CHECK(status IN ('queued','running','retry','succeeded','failed','uncertain','cancelled')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt INTEGER NOT NULL,
    lease_owner TEXT,
    lease_until INTEGER,
    result TEXT CHECK(result IS NULL OR json_valid(result)),
    error_code TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX jobs_due ON integration_jobs(status,next_attempt);
