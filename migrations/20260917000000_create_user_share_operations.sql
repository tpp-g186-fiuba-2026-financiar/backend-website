CREATE TABLE user_share_operations (
    id             SERIAL PRIMARY KEY,
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    share_id       INTEGER NOT NULL REFERENCES shares(id) ON DELETE CASCADE,
    operation_type TEXT NOT NULL CHECK (operation_type IN ('buy', 'sell')),
    quantity       INTEGER NOT NULL CHECK (quantity > 0),
    price          DOUBLE PRECISION NOT NULL CHECK (price > 0),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
 
-- La linea de tiempo de balance historico se arma leyendo todas las
-- operaciones de un usuario ordenadas cronologicamente, por eso el indice
-- principal es (user_id, created_at).
CREATE INDEX user_share_operations_user_created_idx
    ON user_share_operations(user_id, created_at);
 
CREATE INDEX user_share_operations_share_id_idx ON user_share_operations(share_id);
 
