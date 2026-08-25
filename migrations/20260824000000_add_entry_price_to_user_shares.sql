ALTER TABLE user_shares
    ADD COLUMN entry_price DOUBLE PRECISION
        CONSTRAINT user_shares_entry_price_positive CHECK (entry_price IS NULL OR entry_price > 0);
