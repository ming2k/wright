-- Rework workflow storage into active resume state.
--
-- Hard update: existing workflow progress/history from the old schema is
-- discarded. Workflow rows are not package/install history; they are only
-- command-resume state. Successful workflows should not preserve step rows
-- forever, because repeating the same command should revalidate through the
-- build/package/install idempotence layers instead of trusting stale workflow
-- success.

DROP TABLE IF EXISTS workflow_step_events;
DROP TABLE IF EXISTS workflow_steps;
DROP TABLE IF EXISTS workflow_runs;
DROP TABLE IF EXISTS workflows;

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
    failure_json    TEXT,
    started_at      INTEGER,
    finished_at     INTEGER
);

CREATE INDEX workflow_steps_by_workflow_status
    ON workflow_steps(workflow_id, status);
