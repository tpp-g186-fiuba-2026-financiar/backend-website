-- Comparacion de modelos por ticker (GET /user/shares/{ticker}/trends/compare),
-- sin el consenso, que depende del perfil de cada usuario. `complete` es false
-- si algun modelo no respondio: esa respuesta se reusa solo por unos minutos.
CREATE TABLE ticker_compare_cache (
    ticker TEXT PRIMARY KEY,
    payload JSONB NOT NULL,
    complete BOOLEAN NOT NULL,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
