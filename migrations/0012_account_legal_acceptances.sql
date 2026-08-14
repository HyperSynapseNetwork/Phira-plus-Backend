-- Versioned account legal acceptance. Separate from analytics/cookie consent.
CREATE TABLE IF NOT EXISTS account_legal_acceptances (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    terms_version TEXT NOT NULL,
    privacy_version TEXT NOT NULL,
    client_type TEXT NOT NULL CHECK (client_type IN ('ppf','panel','windows','android')),
    source TEXT NOT NULL CHECK (source IN ('phira_login','github_login')),
    accepted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, terms_version, privacy_version)
);
CREATE INDEX IF NOT EXISTS idx_legal_acceptances_user_time
    ON account_legal_acceptances (user_id, accepted_at DESC);
