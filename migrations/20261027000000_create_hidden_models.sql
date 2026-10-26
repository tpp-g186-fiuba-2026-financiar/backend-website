-- Modelos que el admin oculta: no se le muestran a los usuarios.
CREATE TABLE IF NOT EXISTS hidden_models (
    model     TEXT PRIMARY KEY,
    hidden_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Macro arranca oculto: depende de series macro que fallan seguido.
INSERT INTO hidden_models (model) VALUES ('macro') ON CONFLICT DO NOTHING;
