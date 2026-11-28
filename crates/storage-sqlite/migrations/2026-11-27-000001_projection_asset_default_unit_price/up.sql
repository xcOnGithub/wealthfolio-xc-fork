-- Asset facts the kernel reads also include metadata.defaultUnitPrice
-- (fallback unit price for valuation and observed-snapshot cost basis).
-- Recreate projection_asset_update so editing or clearing defaultUnitPrice
-- marks the asset's holders dirty from the beginning.
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
  OR (CASE WHEN json_valid(OLD.metadata) THEN json_extract(OLD.metadata, '$.defaultUnitPrice') END)
     IS NOT (CASE WHEN json_valid(NEW.metadata) THEN json_extract(NEW.metadata, '$.defaultUnitPrice') END)
  OR (CASE WHEN json_valid(OLD.metadata) THEN json_type(OLD.metadata, '$.defaultUnitPrice') END)
     IS NOT (CASE WHEN json_valid(NEW.metadata) THEN json_type(NEW.metadata, '$.defaultUnitPrice') END)
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

-- Carry default-unit-price fallback provenance on generated daily valuations so
-- performance reads flag returns that depend on assumed fallback prices.
ALTER TABLE daily_account_valuation
ADD COLUMN uses_default_unit_price BOOLEAN NOT NULL DEFAULT 0;
