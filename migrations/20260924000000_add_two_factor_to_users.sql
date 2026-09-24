ALTER TABLE users
    ADD COLUMN totp_secret        TEXT,
    ADD COLUMN two_factor_enabled BOOLEAN NOT NULL DEFAULT FALSE;
