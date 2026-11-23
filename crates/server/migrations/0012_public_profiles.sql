-- Public profiles (decided 2026-11-23): on by default, also for existing
-- accounts; the user can turn theirs off in the website's settings. A
-- private profile is left off the leaderboard and its badges show nothing.
ALTER TABLE users ADD COLUMN public_profile BOOLEAN NOT NULL DEFAULT true;
