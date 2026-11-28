DROP TRIGGER IF EXISTS projection_asset_update;
CREATE TRIGGER projection_asset_update AFTER UPDATE OF kind, quote_ccy, instrument_type, instrument_symbol, metadata ON assets
WHEN OLD.kind IS NOT NEW.kind
  OR OLD.quote_ccy IS NOT NEW.quote_ccy
  OR OLD.instrument_type IS NOT NEW.instrument_type
  OR ((OLD.kind = 'FX' OR NEW.kind = 'FX') AND OLD.instrument_symbol IS NOT NEW.instrument_symbol)
  OR (CASE WHEN json_valid(OLD.metadata) THEN json_extract(OLD.metadata, '$.option') END)
     IS NOT (CASE WHEN json_valid(NEW.metadata) THEN json_extract(NEW.metadata, '$.option') END)
  OR (CASE WHEN json_valid(OLD.metadata) THEN json_extract(OLD.metadata, '$.contractMultiplier') END)
     IS NOT (CASE WHEN json_valid(NEW.metadata) THEN json_extract(NEW.metadata, '$.contractMultiplier') END)
BEGIN
    INSERT INTO projection_state (scope, dirty_from, version)
    VALUES ('a:' || NEW.id, '0001-01-01', 1)
    ON CONFLICT (scope) DO UPDATE SET
        dirty_from = '0001-01-01',
        version = projection_state.version + 1;
    INSERT INTO projection_state (scope, dirty_from, version)
    SELECT '@all', '0001-01-01', 1
    WHERE (OLD.kind = 'FX' OR NEW.kind = 'FX')
      AND (OLD.kind IS NOT NEW.kind
        OR OLD.quote_ccy IS NOT NEW.quote_ccy
        OR OLD.instrument_symbol IS NOT NEW.instrument_symbol)
    ON CONFLICT (scope) DO UPDATE SET
        dirty_from = '0001-01-01',
        version = projection_state.version + 1;
END;

ALTER TABLE daily_account_valuation
DROP COLUMN uses_default_unit_price;
