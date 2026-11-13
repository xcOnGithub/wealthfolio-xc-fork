CREATE TABLE account_statement_history (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    statement_date TEXT NOT NULL,
    payload TEXT NOT NULL,
    revision TEXT NOT NULL,
    PRIMARY KEY (account_id, statement_date)
);
