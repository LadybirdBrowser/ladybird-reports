-- The title and description in Reports are cached GitHub issue fields.
-- Prefer the last title received from GitHub over any local edit made before
-- this migration. Descriptions are refreshed from GitHub on the next issue
-- view or issue webhook.
UPDATE issues SET title = github_title WHERE title IS DISTINCT FROM github_title;

ALTER TABLE issues DROP COLUMN github_title;
