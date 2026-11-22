-- Add optional household member attribution and structured tax-advantaged
-- category to contribution limits for per-member family tracking.
ALTER TABLE contribution_limits ADD COLUMN member_name TEXT;
ALTER TABLE contribution_limits ADD COLUMN limit_category TEXT;
