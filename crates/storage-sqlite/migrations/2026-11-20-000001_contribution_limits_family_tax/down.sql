-- Revert household member attribution and category columns.
ALTER TABLE contribution_limits DROP COLUMN limit_category;
ALTER TABLE contribution_limits DROP COLUMN member_name;
