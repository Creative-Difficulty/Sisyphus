-- Which version of the repository matcher (`github::MATCHER`) made the
-- last automatic lookup. Misses of older versions are looked up again, so
-- a matcher that finds more (like contributed repositories) reaches
-- projects it missed before.
ALTER TABLE projects ADD COLUMN repo_checked_by INTEGER NOT NULL DEFAULT 0;
