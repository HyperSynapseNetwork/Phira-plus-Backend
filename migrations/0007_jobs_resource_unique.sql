-- Job mutual-exclusion at the DB level (Stop-ship).
--
-- Replaces the application-level `COUNT active → INSERT` race: two concurrent
-- `POST /jobs` for the same resource_key could both pass the COUNT and both
-- INSERT (double `pmp.update.apply`). A partial UNIQUE INDEX makes the
-- exclusion atomic — the second INSERT hits a unique violation and the runner
-- maps it to 409 `already running`.
--
-- Predicate uses `resource_key <> ''` (not `IS NOT NULL`) because the column
-- is `TEXT NOT NULL DEFAULT ''`; the empty string means "no exclusion" and
-- mirrors the runner's `Some(resource_key)` guard.

DROP INDEX idx_jobs_resource_key_active;

CREATE UNIQUE INDEX idx_jobs_resource_key_active_unique
    ON jobs (resource_key)
    WHERE resource_key <> '' AND state IN ('queued', 'running');
