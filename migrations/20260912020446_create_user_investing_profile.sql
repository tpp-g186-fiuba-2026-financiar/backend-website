CREATE TABLE IF NOT EXISTS user_investing_profiles (
    id            SERIAL PRIMARY KEY,
    user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    risk_profile  TEXT NOT NULL
                  CHECK (risk_profile IN ('conservative', 'moderate', 'aggressive')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at    TIMESTAMPTZ NOT NULL
                  GENERATED ALWAYS AS (created_at + INTERVAL '6 months') STORED
);

CREATE INDEX IF NOT EXISTS idx_user_investing_profiles_user_id
    ON user_investing_profiles (user_id, created_at DESC);