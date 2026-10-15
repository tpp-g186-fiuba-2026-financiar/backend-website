-- Reseteo de contrasena por mail: se guarda solo el hash (SHA-256) del token
-- que viaja en el link, igual que en la verificacion de mail.
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS password_reset_token_hash TEXT,
    ADD COLUMN IF NOT EXISTS password_reset_expires_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS password_reset_sent_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_users_password_reset_token_hash
    ON users (password_reset_token_hash)
    WHERE password_reset_token_hash IS NOT NULL;
