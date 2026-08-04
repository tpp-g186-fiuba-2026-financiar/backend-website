-- Tablero de retro del equipo (no es parte del dominio del TP, es una
-- herramienta interna). A proposito no guarda quien escribio cada tarjeta:
-- la idea es que sea anonimo dentro del equipo.
CREATE TABLE retro_cards (
    id SERIAL PRIMARY KEY,
    column_name TEXT NOT NULL CHECK (column_name IN ('bien', 'mejorar', 'acciones', 'preguntas')),
    content TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Cada vez que se "archiva" una retro se guarda una foto de como quedo el
-- tablero (columnas + tarjetas) en ese momento, con una etiqueta (por
-- default la fecha) y el numero de sprint, y se borran las tarjetas activas
-- para la proxima.
CREATE TABLE retro_archives (
    id SERIAL PRIMARY KEY,
    label TEXT NOT NULL,
    sprint_number INTEGER NOT NULL,
    snapshot JSONB NOT NULL,
    archived_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Fila unica (id fijo = 1) con el numero de sprint actual. Arranca en 15
-- porque el equipo ya venia contando sprints antes de este tablero.
CREATE TABLE retro_sprint (
    id INTEGER PRIMARY KEY DEFAULT 1,
    current_sprint INTEGER NOT NULL,
    CONSTRAINT retro_sprint_single_row CHECK (id = 1)
);
INSERT INTO retro_sprint (id, current_sprint) VALUES (1, 15);
