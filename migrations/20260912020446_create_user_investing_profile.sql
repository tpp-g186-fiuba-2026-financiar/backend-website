CREATE TABLE IF NOT EXISTS user_investing_profiles (
    id            SERIAL PRIMARY KEY,
    user_id       INTEGER REFERENCES users(id) ON DELETE CASCADE,
    risk_profile  TEXT CHECK (risk_profile IN ('conservative', 'moderate', 'aggressive')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at    TIMESTAMPTZ NOT NULL DEFAULT (NOW() + INTERVAL '6 months'),
    is_active     BOOLEAN NOT NULL DEFAULT TRUE
)