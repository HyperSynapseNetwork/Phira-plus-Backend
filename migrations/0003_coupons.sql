-- PPB Phase B coupons (design §18.14, contract §17).
-- Redemption must execute an Action (V1: admin create/revoke; redemption later).
CREATE TABLE coupons (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    code        TEXT NOT NULL UNIQUE,
    action_type TEXT NOT NULL,
    payload     JSONB NOT NULL DEFAULT '{}'::jsonb,
    max_uses    INTEGER NOT NULL DEFAULT 1,
    used_count  INTEGER NOT NULL DEFAULT 0,
    created_by  UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at  TIMESTAMPTZ
);
CREATE INDEX idx_coupons_code ON coupons (code);
