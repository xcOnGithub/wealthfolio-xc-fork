-- Profile-local notification metadata. Delivery claims must never enter device-sync.
CREATE TABLE family_money_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    body TEXT NOT NULL CHECK (json_valid(body))
);
