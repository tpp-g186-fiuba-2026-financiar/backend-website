CREATE TABLE IF NOT EXISTS shares (
    id          SERIAL PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    ticker      TEXT    NOT NULL,
    quantity    INTEGER NOT NULL CHECK (quantity > 0),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, ticker)
);

CREATE INDEX shares_user_id_idx ON shares(user_id);
