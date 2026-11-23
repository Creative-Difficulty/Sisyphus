-- A connected GitHub account (decided 2026-11-23). The token has no extra
-- scopes (public data only) and is used to find each project's public
-- repository. Disconnecting deletes it.
ALTER TABLE users ADD COLUMN github_id BIGINT;
ALTER TABLE users ADD COLUMN github_login TEXT;
ALTER TABLE users ADD COLUMN github_token TEXT;

-- A user's project (the folder name its sessions report) and its links.
CREATE TABLE projects (
    user_id         BIGINT  NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name            TEXT    NOT NULL,
    -- The project's repository, found on GitHub or set by the user.
    repo_url        TEXT,
    -- The user set (or cleared) the repository; automatic matching leaves
    -- it alone from then on.
    repo_manual     BOOLEAN NOT NULL DEFAULT false,
    -- The last automatic lookup (match or miss), unix ms.
    repo_checked_ms BIGINT,
    -- Where the finished project can be tried, set by the user.
    demo_url        TEXT,
    PRIMARY KEY (user_id, name)
);
