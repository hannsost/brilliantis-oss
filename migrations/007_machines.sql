-- Operator-owned service principals never share human passwords, roles or sessions.
CREATE TABLE machine_identities (
    name TEXT PRIMARY KEY,
    current_generation TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE machine_tokens (
    machine TEXT NOT NULL REFERENCES machine_identities(name),
    generation TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    scopes TEXT NOT NULL CHECK(json_valid(scopes)),
    request TEXT NOT NULL CHECK(json_valid(request)),
    output_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN (0,1)),
    PRIMARY KEY(machine,generation)
);
CREATE UNIQUE INDEX machine_one_active_token ON machine_tokens(machine) WHERE revoked=0;
