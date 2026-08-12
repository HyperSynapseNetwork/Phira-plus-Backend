-- PPB admin tasks (design §9.4 / contract §17 Admin Tasks).
-- Manual operations that require an admin to complete (e.g. coupon-based
-- account unlocks). Seeded by coupon creation or other flows; completed by
-- `POST /admin/jobs/tasks/{id}/complete`.
CREATE TABLE admin_tasks (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source       TEXT NOT NULL DEFAULT 'manual' CHECK (source IN ('coupon', 'manual')),
    source_id    UUID,
    task_type    TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'completed', 'failed')),
    payload      JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_by   UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ
);
CREATE INDEX idx_admin_tasks_status ON admin_tasks (status);
