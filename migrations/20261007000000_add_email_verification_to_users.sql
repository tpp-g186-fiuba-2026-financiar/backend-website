ALTER TABLE users
    ADD COLUMN email_verified                   BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN email_verification_token_hash    TEXT,
    ADD COLUMN email_verification_expires_at    TIMESTAMPTZ,
    ADD COLUMN email_verification_sent_at       TIMESTAMPTZ;

-- Los usuarios que ya existian se consideran verificados para no bloquearles el login.
UPDATE users SET email_verified = TRUE;

CREATE UNIQUE INDEX users_email_verification_token_hash_idx
    ON users (email_verification_token_hash)
    WHERE email_verification_token_hash IS NOT NULL;
