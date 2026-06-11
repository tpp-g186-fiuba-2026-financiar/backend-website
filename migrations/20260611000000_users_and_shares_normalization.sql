ALTER TABLE shares
    DROP COLUMN user_id,
    DROP COLUMN quantity,
    DROP COLUMN created_at;

ALTER TABLE shares
    ADD CONSTRAINT shares_ticker_unique UNIQUE (ticker);

DROP INDEX IF EXISTS shares_user_id_idx;

CREATE TABLE IF NOT EXISTS user_shares (
    id         SERIAL PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    share_id   INTEGER NOT NULL REFERENCES shares(id) ON DELETE CASCADE,
    quantity   INTEGER NOT NULL CHECK (quantity > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, share_id)
);

CREATE INDEX user_shares_user_id_idx ON user_shares(user_id);
CREATE INDEX user_shares_share_id_idx ON user_shares(share_id);