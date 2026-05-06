-- Workflow / Step / Run model.
--
-- A workflow is a content-addressed plan (id derived from canonical inputs).
-- A step is a single resumable unit of work belonging to a workflow.
-- A run is one attempt to drive a workflow forward; multiple runs share steps.
--
-- The legacy execution_sessions tables remain for now; they are removed in
-- a follow-up migration after callers are migrated to this model.

CREATE TABLE workflows (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    inputs_json TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);

CREATE TABLE workflow_steps (
    id              TEXT PRIMARY KEY,
    workflow_id     TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL,
    inputs_json     TEXT NOT NULL,
    depends_on_json TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN
                       ('pending','running','succeeded','failed','skipped')),
    attempt         INTEGER NOT NULL DEFAULT 0,
    outputs_json    TEXT,
    error_text      TEXT,
    started_at      INTEGER,
    finished_at     INTEGER
);

CREATE INDEX workflow_steps_by_workflow_status
    ON workflow_steps(workflow_id, status);

CREATE TABLE workflow_runs (
    id              TEXT PRIMARY KEY,
    workflow_id     TEXT NOT NULL REFERENCES workflows(id),
    started_at      INTEGER NOT NULL,
    last_active_at  INTEGER NOT NULL,
    terminal_status TEXT
);

CREATE INDEX workflow_runs_by_workflow
    ON workflow_runs(workflow_id, started_at);

CREATE TABLE workflow_step_events (
    run_id   TEXT NOT NULL REFERENCES workflow_runs(id),
    step_id  TEXT NOT NULL REFERENCES workflow_steps(id),
    event    TEXT NOT NULL,
    at       INTEGER NOT NULL,
    detail   TEXT,
    PRIMARY KEY (run_id, step_id, at, event)
);
