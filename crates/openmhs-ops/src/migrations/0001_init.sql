-- RFC 0005 §6. Events are the truth; every table marked "derived" is rebuilt from them.

CREATE TABLE events (
    tenant         TEXT NOT NULL,
    event_id       TEXT NOT NULL,
    type           TEXT NOT NULL,
    source_id      TEXT NOT NULL,
    boot_id        TEXT,
    seq            INTEGER,
    observed_ns    INTEGER,
    clock_domain   TEXT NOT NULL,
    uncertainty_ms REAL,
    ingested_ns    INTEGER NOT NULL,
    site           TEXT,
    robot          TEXT NOT NULL,
    task_id        TEXT,
    task_type      TEXT,
    episode_id     TEXT,
    action_id      TEXT,
    goal_id        TEXT,
    trace_id       TEXT,
    manifest       TEXT,
    reported       TEXT,
    verified       TEXT,
    severity       TEXT,
    category       TEXT,
    code           TEXT,
    component      TEXT,
    provenance     TEXT NOT NULL,
    content_hash   TEXT NOT NULL,
    body           TEXT NOT NULL,
    PRIMARY KEY (tenant, event_id)
) WITHOUT ROWID;
CREATE INDEX events_robot_time ON events(tenant, robot, observed_ns);
CREATE INDEX events_task ON events(tenant, task_id) WHERE task_id IS NOT NULL;
CREATE INDEX events_goal ON events(tenant, goal_id) WHERE goal_id IS NOT NULL;
CREATE INDEX events_type_time ON events(tenant, type, observed_ns);

CREATE TABLE event_conflicts (
    tenant       TEXT NOT NULL,
    event_id     TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    body         TEXT NOT NULL,
    ingested_ns  INTEGER NOT NULL,
    PRIMARY KEY (tenant, event_id, content_hash)
) WITHOUT ROWID;

CREATE TABLE manifests (
    digest        TEXT PRIMARY KEY,
    release       TEXT,
    body          TEXT NOT NULL,
    first_seen_ns INTEGER NOT NULL
);

CREATE TABLE artifacts (
    tenant       TEXT NOT NULL,
    artifact_id  TEXT NOT NULL,
    uri          TEXT NOT NULL,
    media_type   TEXT,
    bytes        INTEGER,
    start_ns     INTEGER,
    end_ns       INTEGER,
    robot        TEXT,
    content_hash TEXT NOT NULL,
    body         TEXT NOT NULL,
    PRIMARY KEY (tenant, artifact_id)
) WITHOUT ROWID;

-- derived
CREATE TABLE rollouts (
    tenant       TEXT NOT NULL,
    robot        TEXT NOT NULL,
    from_ns      INTEGER NOT NULL,
    to_ns        INTEGER,
    manifest     TEXT NOT NULL,
    source_event TEXT NOT NULL,
    PRIMARY KEY (tenant, robot, from_ns)
);

-- derived
CREATE TABLE task_attempts (
    tenant         TEXT NOT NULL,
    robot          TEXT NOT NULL,
    task_id        TEXT NOT NULL,
    task_type      TEXT NOT NULL,
    site           TEXT,
    manifest       TEXT,
    manifest_basis TEXT,
    started_ns     INTEGER,
    ended_ns       INTEGER,
    reported       TEXT,
    verified       TEXT,
    interventions  INTEGER NOT NULL,
    close_reason   TEXT,
    rules_version  TEXT NOT NULL,
    PRIMARY KEY (tenant, robot, task_id)
);
CREATE INDEX task_attempts_type ON task_attempts(tenant, task_type, manifest, started_ns);

-- derived
CREATE TABLE incidents (
    tenant        TEXT NOT NULL,
    incident_id   TEXT NOT NULL,
    rules_version TEXT NOT NULL,
    robot         TEXT NOT NULL,
    site          TEXT,
    task_id       TEXT NOT NULL,
    task_type     TEXT NOT NULL,
    opened_ns     INTEGER,
    closed_ns     INTEGER,
    triggers      TEXT NOT NULL,
    fingerprint   TEXT NOT NULL,
    reported      TEXT,
    verified      TEXT,
    manifest      TEXT,
    PRIMARY KEY (tenant, incident_id)
);
CREATE INDEX incidents_fingerprint ON incidents(tenant, fingerprint, opened_ns);
CREATE INDEX incidents_robot ON incidents(tenant, robot, opened_ns);

-- derived
CREATE TABLE incident_members (
    tenant      TEXT NOT NULL,
    incident_id TEXT NOT NULL,
    event_id    TEXT NOT NULL,
    basis       TEXT NOT NULL,
    detail      TEXT NOT NULL,
    PRIMARY KEY (tenant, incident_id, event_id)
);

-- human state: never rebuilt, kept by incident id
CREATE TABLE incident_state (
    tenant      TEXT NOT NULL,
    incident_id TEXT NOT NULL,
    owner       TEXT,
    status      TEXT NOT NULL,
    updated_ns  INTEGER NOT NULL,
    PRIMARY KEY (tenant, incident_id)
);

CREATE TABLE annotations (
    tenant        TEXT NOT NULL,
    annotation_id TEXT NOT NULL,
    target        TEXT NOT NULL,
    author        TEXT NOT NULL,
    provenance    TEXT NOT NULL,
    body          TEXT NOT NULL,
    created_ns    INTEGER NOT NULL,
    PRIMARY KEY (tenant, annotation_id)
);
