-- PPB Phase B schema: config snapshots, public content, PPF build config,
-- chart snapshots (Aggregator), PPB runtime config overrides.

-- Full config snapshots (PMP YAML / PPF build config) for snapshot/rollback.
CREATE TABLE config_snapshots (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    scope       TEXT NOT NULL, -- 'pmp' | 'ppf_build'
    content     TEXT NOT NULL, -- full YAML/JSON snapshot
    note        TEXT NOT NULL DEFAULT '',
    created_by  UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    restored_at TIMESTAMPTZ
);
CREATE INDEX idx_config_snapshots_scope ON config_snapshots (scope, created_at DESC);

-- Public runtime content (site / announcements / downloads / nodes).
CREATE TABLE public_content (
    key         TEXT PRIMARY KEY,
    content     JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_by  UUID REFERENCES users(id) ON DELETE SET NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- PPF build/SEO config (single row).
CREATE TABLE ppf_build_config (
    id          SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    revision    BIGINT NOT NULL DEFAULT 0,
    content     JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_by  UUID REFERENCES users(id) ON DELETE SET NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- PPB runtime config overrides (single row; merged over boot-time TOML).
CREATE TABLE ppb_runtime_overrides (
    id          SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    content     JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_by  UUID REFERENCES users(id) ON DELETE SET NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Chart popularity snapshots (Phira Aggregator / TopChart, hourly).
CREATE TABLE chart_snapshots (
    chart_id     BIGINT NOT NULL,
    record_count BIGINT NOT NULL,
    snapshot_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (chart_id, snapshot_at)
);
CREATE INDEX idx_chart_snapshots_time ON chart_snapshots (snapshot_at);
