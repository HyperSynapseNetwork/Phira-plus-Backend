-- PPB Gate 3 (S-3): Replay identity is (round_uuid, player_phira_id).
-- Visibility overrides and share links bind to the pair; share tokens grant a
-- single Replay (no cross-Replay access).

ALTER TABLE replay_overrides
    ADD COLUMN player_phira_id BIGINT NOT NULL DEFAULT 0;
ALTER TABLE replay_overrides
    DROP CONSTRAINT IF EXISTS replay_overrides_pmp_replay_id_key;
ALTER TABLE replay_overrides
    ADD CONSTRAINT replay_overrides_pair_key UNIQUE (pmp_replay_id, player_phira_id);

ALTER TABLE replay_share_links
    ADD COLUMN player_phira_id BIGINT NOT NULL DEFAULT 0;
