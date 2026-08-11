-- PPB initial schema (Phase A).
-- Data ownership: PPB owns identity / policy / control / community / preferences / notifications / audit.
-- PMP owns gameplay facts (rooms/rounds/touches/judges/replay). PPB never stores Replay content.

CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- ─────────────────────────────────────────────────────────────
-- users
-- ─────────────────────────────────────────────────────────────
CREATE TABLE users (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    phira_id        BIGINT NOT NULL UNIQUE,
    username_cache  TEXT NOT NULL DEFAULT '',
    avatar_cache    TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'active', -- active | disabled
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at    TIMESTAMPTZ
);

CREATE INDEX idx_users_phira_id ON users (phira_id);
CREATE INDEX idx_users_last_seen ON users (last_seen_at);

-- ─────────────────────────────────────────────────────────────
-- user_identities — provider = phira | github
-- ─────────────────────────────────────────────────────────────
CREATE TABLE user_identities (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider      TEXT NOT NULL CHECK (provider IN ('phira', 'github')),
    provider_id   TEXT NOT NULL,
    provider_name TEXT NOT NULL DEFAULT '',
    linked_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (provider, provider_id)
);

CREATE INDEX idx_user_identities_user ON user_identities (user_id);

-- ─────────────────────────────────────────────────────────────
-- phira_credentials — encrypted Phira refresh token (never plaintext).
-- State drives PHIRA_REAUTH_REQUIRED.
-- ─────────────────────────────────────────────────────────────
CREATE TABLE phira_credentials (
    user_id                  UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    refresh_token_ciphertext BYTEA NOT NULL,
    refresh_expires_at       TIMESTAMPTZ NOT NULL,
    state                    TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'reauth_required', 'revoked')),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ─────────────────────────────────────────────────────────────
-- sessions — principal_type user | root; client_type ppf | panel | windows | android
-- ─────────────────────────────────────────────────────────────
CREATE TABLE sessions (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    principal_type TEXT NOT NULL CHECK (principal_type IN ('user', 'root')),
    user_id       UUID REFERENCES users(id) ON DELETE CASCADE, -- NULL for root
    client_type   TEXT NOT NULL CHECK (client_type IN ('ppf', 'panel', 'windows', 'android')),
    refresh_hash  TEXT NOT NULL, -- HMAC/hash of server-side session refresh secret
    device_name   TEXT NOT NULL DEFAULT '',
    ip            TEXT NOT NULL DEFAULT '',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at    TIMESTAMPTZ NOT NULL,
    revoked_at    TIMESTAMPTZ,
    last_seen_at  TIMESTAMPTZ
);

CREATE INDEX idx_sessions_user ON sessions (user_id);
CREATE INDEX idx_sessions_refresh_hash ON sessions (refresh_hash);

-- ─────────────────────────────────────────────────────────────
-- root_credentials — Root is a local emergency principal, NOT in users.
-- ─────────────────────────────────────────────────────────────
CREATE TABLE root_credentials (
    id                    SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    password_hash         TEXT NOT NULL,
    must_change_password  BOOLEAN NOT NULL DEFAULT TRUE,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ─────────────────────────────────────────────────────────────
-- groups — system_kind = admin_scope for Administrators
-- ─────────────────────────────────────────────────────────────
CREATE TABLE groups (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL DEFAULT '',
    system_kind TEXT CHECK (system_kind IN ('admin_scope')), -- NULL for ordinary groups
    is_default  BOOLEAN NOT NULL DEFAULT FALSE,
    protected   BOOLEAN NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_groups_default ON groups (is_default) WHERE is_default = TRUE;

CREATE TABLE group_members (
    group_id  UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id   UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, user_id)
);

-- `*:*` is Root-only. The database rejects it for ANY group row (Root does not
-- live in groups). admin_scope is modeled as a flag, not a stored `*:*`.
CREATE TABLE group_permissions (
    group_id   UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    permission TEXT NOT NULL CHECK (permission <> '*:*'),
    PRIMARY KEY (group_id, permission)
);

-- ─────────────────────────────────────────────────────────────
-- user_profiles
-- ─────────────────────────────────────────────────────────────
CREATE TABLE user_profiles (
    user_id               UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    bio                   TEXT NOT NULL DEFAULT '',
    background_url        TEXT NOT NULL DEFAULT '',
    profile_visibility    TEXT NOT NULL DEFAULT 'public' CHECK (profile_visibility IN ('public', 'friends', 'private')),
    show_online_status    BOOLEAN NOT NULL DEFAULT TRUE,
    show_recent_activity  BOOLEAN NOT NULL DEFAULT TRUE,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ─────────────────────────────────────────────────────────────
-- social — bidirectional friends (not Phira follow)
-- ─────────────────────────────────────────────────────────────
CREATE TABLE friend_requests (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    from_user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    to_user_id   UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status       TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'declined', 'cancelled')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    responded_at TIMESTAMPTZ,
    UNIQUE (from_user_id, to_user_id)
);

-- Normalized unique pair (user_a < user_b) so A-B and B-A are the same row.
CREATE TABLE friendships (
    user_a     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    user_b     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_a, user_b),
    CHECK (user_a < user_b)
);

CREATE TABLE user_blocks (
    blocker_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    blocked_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (blocker_id, blocked_id)
);

-- ─────────────────────────────────────────────────────────────
-- replay — POLICY ONLY. Never Replay content/files.
-- ─────────────────────────────────────────────────────────────
CREATE TABLE replay_overrides (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    pmp_replay_id TEXT NOT NULL,          -- round identity (PMP)
    owner_user_id UUID REFERENCES users(id) ON DELETE CASCADE,
    visibility    TEXT NOT NULL DEFAULT 'inherit' CHECK (visibility IN ('inherit','public','friends','private','unlisted','custom')),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (pmp_replay_id)
);

CREATE TABLE replay_acl (
    id        UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    replay_id UUID NOT NULL REFERENCES replay_overrides(id) ON DELETE CASCADE,
    user_id   UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    effect    TEXT NOT NULL CHECK (effect IN ('allow', 'deny')),
    UNIQUE (replay_id, user_id)
);

CREATE TABLE replay_share_links (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    replay_round  TEXT NOT NULL,
    token_hash    TEXT NOT NULL UNIQUE, -- SHA-256 of opaque token; raw token never stored
    created_by    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at    TIMESTAMPTZ,
    revoked_at    TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ─────────────────────────────────────────────────────────────
-- user_preferences — JSONB + revision (optimistic concurrency)
-- ─────────────────────────────────────────────────────────────
CREATE TABLE user_preferences (
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    namespace  TEXT NOT NULL CHECK (namespace IN ('common','ppf','panel','experiments')),
    revision   BIGINT NOT NULL DEFAULT 0,
    json_data  JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, namespace)
);

-- ─────────────────────────────────────────────────────────────
-- notifications — event/inbox separated
-- ─────────────────────────────────────────────────────────────
CREATE TABLE notification_events (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    type          TEXT NOT NULL,
    actor_user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    payload       JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE user_notifications (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id     UUID NOT NULL REFERENCES notification_events(id) ON DELETE CASCADE,
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    read_at      TIMESTAMPTZ,
    dismissed_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (event_id, user_id)
);

CREATE INDEX idx_user_notifications_user ON user_notifications (user_id, created_at DESC);

-- Push endpoints — tokens encrypted at rest.
CREATE TABLE push_endpoints (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id           UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id         TEXT NOT NULL DEFAULT '',
    channel           TEXT NOT NULL CHECK (channel IN ('web_push','fcm','wns')),
    endpoint_ciphertext BYTEA NOT NULL,
    platform          TEXT NOT NULL DEFAULT '',
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at      TIMESTAMPTZ,
    disabled_at       TIMESTAMPTZ,
    UNIQUE (user_id, device_id)
);

-- ─────────────────────────────────────────────────────────────
-- audit_events — 90-day retention
-- ─────────────────────────────────────────────────────────────
CREATE TABLE audit_events (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    occurred_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    principal_type      TEXT NOT NULL CHECK (principal_type IN ('user','root')),
    actor_user_id       UUID REFERENCES users(id) ON DELETE SET NULL,
    actor_session_id    UUID,
    action              TEXT NOT NULL,
    resource_type       TEXT NOT NULL DEFAULT '',
    resource_id         TEXT NOT NULL DEFAULT '',
    parameters_redacted JSONB NOT NULL DEFAULT '{}'::jsonb,
    result              TEXT NOT NULL DEFAULT 'success' CHECK (result IN ('success','denied','error')),
    error_code          TEXT NOT NULL DEFAULT '',
    request_id          TEXT NOT NULL DEFAULT '',
    command_id          TEXT NOT NULL DEFAULT '',
    ip                  TEXT NOT NULL DEFAULT '',
    user_agent          TEXT NOT NULL DEFAULT ''
);

CREATE INDEX idx_audit_occurred ON audit_events (occurred_at);
CREATE INDEX idx_audit_actor ON audit_events (actor_user_id);
CREATE INDEX idx_audit_action ON audit_events (action);

-- ─────────────────────────────────────────────────────────────
-- command_runs — Command Broker record
-- ─────────────────────────────────────────────────────────────
CREATE TABLE command_runs (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    action             TEXT NOT NULL,
    actor              TEXT NOT NULL DEFAULT '', -- principal id or root
    resource_key       TEXT NOT NULL DEFAULT '',
    arguments_redacted JSONB NOT NULL DEFAULT '{}'::jsonb,
    status             TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued','running','succeeded','failed','cancelled')),
    started_at         TIMESTAMPTZ,
    finished_at        TIMESTAMPTZ,
    result_summary     TEXT NOT NULL DEFAULT '',
    error_code         TEXT NOT NULL DEFAULT ''
);

CREATE INDEX idx_command_runs_resource ON command_runs (resource_key, started_at);

-- ─────────────────────────────────────────────────────────────
-- jobs — long-running tasks
-- ─────────────────────────────────────────────────────────────
CREATE TABLE jobs (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    type        TEXT NOT NULL,
    state       TEXT NOT NULL DEFAULT 'queued' CHECK (state IN ('queued','running','succeeded','failed','cancelled')),
    progress    REAL,
    stage       TEXT NOT NULL DEFAULT '',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at  TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    error       TEXT NOT NULL DEFAULT ''
);

-- ─────────────────────────────────────────────────────────────
-- automation — runbooks (Phase B uses; schema defined now)
-- ─────────────────────────────────────────────────────────────
CREATE TABLE runbooks (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    definition  JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_by  UUID REFERENCES users(id) ON DELETE SET NULL,
    updated_by  UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE runbook_runs (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    runbook_id          UUID NOT NULL REFERENCES runbooks(id) ON DELETE CASCADE,
    definition_snapshot JSONB NOT NULL DEFAULT '{}'::jsonb,
    arguments_redacted  JSONB NOT NULL DEFAULT '{}'::jsonb,
    actor               TEXT NOT NULL DEFAULT '',
    status              TEXT NOT NULL DEFAULT 'queued',
    started_at          TIMESTAMPTZ,
    finished_at         TIMESTAMPTZ
);
