-- Provider state is local to this profile, deliberately outside device-sync outboxes.
CREATE TABLE plaid_connections (
  id TEXT PRIMARY KEY NOT NULL,
  environment TEXT NOT NULL,
  item_id TEXT,
  status TEXT NOT NULL,
  body TEXT NOT NULL,
  UNIQUE(environment, item_id)
);
CREATE TABLE plaid_accounts (
  id TEXT PRIMARY KEY NOT NULL,
  connection_id TEXT NOT NULL REFERENCES plaid_connections(id) ON DELETE CASCADE,
  external_id TEXT NOT NULL,
  account_id TEXT NOT NULL UNIQUE REFERENCES accounts(id) ON DELETE CASCADE,
  body TEXT NOT NULL,
  UNIQUE(connection_id, external_id)
);
