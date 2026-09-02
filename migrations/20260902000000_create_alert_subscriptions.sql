CREATE TABLE alert_subscriptions (
    id         SERIAL PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- NULL = suscripcion a "toda la cartera" (cualquier ticker que el usuario
    -- tenga declarado en user_shares), no NULL = suscripcion a un ticker puntual.
    share_id   INTEGER REFERENCES shares(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Postgres no deduplica NULLs en un UNIQUE normal, asi que se necesitan dos
-- indices parciales para evitar duplicados en ambos casos.
CREATE UNIQUE INDEX alert_subscriptions_user_share_unique
    ON alert_subscriptions(user_id, share_id) WHERE share_id IS NOT NULL;

CREATE UNIQUE INDEX alert_subscriptions_user_portfolio_unique
    ON alert_subscriptions(user_id) WHERE share_id IS NULL;

CREATE INDEX alert_subscriptions_share_id_idx ON alert_subscriptions(share_id);
