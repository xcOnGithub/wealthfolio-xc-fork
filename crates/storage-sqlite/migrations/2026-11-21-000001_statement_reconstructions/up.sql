CREATE TABLE account_statement_reconstructions (
    account_id TEXT NOT NULL,
    through_date TEXT NOT NULL,
    payload TEXT NOT NULL,
    revision TEXT NOT NULL,
    PRIMARY KEY (account_id, through_date),
    FOREIGN KEY (account_id, through_date)
        REFERENCES account_statement_history(account_id, statement_date) ON DELETE CASCADE
);
-- A monthly-only replacement must not leave daily facts tied to an old checkpoint.
CREATE TRIGGER invalidate_statement_reconstruction AFTER UPDATE OF payload ON account_statement_history
WHEN OLD.payload != NEW.payload
BEGIN
    DELETE FROM account_statement_reconstructions
    WHERE account_id = OLD.account_id AND through_date = OLD.statement_date;
END;
