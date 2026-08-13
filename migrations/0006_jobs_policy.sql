-- Job policy hardening (Stop-ship):
--   1. `resource_key` — parallelism-exclusion for long jobs (pmp.update.* -> "server").
--   2. `not_implemented` terminal state — stub jobs (ppf.build / backup) must
--      never fake `succeeded`.

ALTER TABLE jobs ADD COLUMN resource_key TEXT NOT NULL DEFAULT '';
CREATE INDEX idx_jobs_resource_key_active
    ON jobs (resource_key)
    WHERE state IN ('queued', 'running');

ALTER TABLE jobs DROP CONSTRAINT jobs_state_check;
ALTER TABLE jobs ADD CONSTRAINT jobs_state_check
    CHECK (state IN ('queued', 'running', 'succeeded', 'failed', 'cancelled', 'not_implemented'));
