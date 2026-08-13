-- Console ViewModel (§18.10): `command_runs` gains the raw command text and a
-- `scope` discriminator so `/admin/commands/execute` and `/admin/commands`
-- (history) share one `CommandRun` shape:
--   command_id / command / action / status / output / error / executed_at /
--   principal / scope.

ALTER TABLE command_runs ADD COLUMN command TEXT NOT NULL DEFAULT '';
ALTER TABLE command_runs ADD COLUMN scope TEXT NOT NULL DEFAULT 'personal'
    CHECK (scope IN ('personal', 'server'));
