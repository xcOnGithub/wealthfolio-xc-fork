-- Preserve explicit provider basis knowledge in normalized snapshot reads.
ALTER TABLE snapshot_positions ADD COLUMN cost_basis_status TEXT;
