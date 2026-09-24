CREATE TABLE IF NOT EXISTS user_investing_profiles (
    id            SERIAL PRIMARY KEY,
    user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    risk_profile  TEXT NOT NULL
                  CHECK (risk_profile IN ('conservative', 'moderate', 'aggressive')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at    TIMESTAMPTZ NOT NULL DEFAULT NOW() 
);

CREATE INDEX IF NOT EXISTS idx_user_investing_profiles_user_id
    ON user_investing_profiles (user_id, created_at DESC);

CREATE OR REPLACE FUNCTION set_investing_profile_expiry()
RETURNS TRIGGER AS $$
BEGIN
    NEW.expires_at := NEW.created_at + INTERVAL '6 months';
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_set_investing_profile_expiry
    BEFORE INSERT OR UPDATE OF created_at ON user_investing_profiles
    FOR EACH ROW
    EXECUTE FUNCTION set_investing_profile_expiry();