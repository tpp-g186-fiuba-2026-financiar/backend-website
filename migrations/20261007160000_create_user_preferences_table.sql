CREATE TABLE IF NOT EXISTS user_preferences (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    stock   TEXT NOT NULL,
    model   TEXT,
    PRIMARY KEY (user_id, stock)
);
