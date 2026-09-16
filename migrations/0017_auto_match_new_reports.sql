-- Existing reports are indexed for search, but only reports submitted after
-- this migration participate in automatic issue matching.
ALTER TABLE reports
    ADD COLUMN auto_match_eligible boolean NOT NULL DEFAULT false;

ALTER TABLE reports
    ALTER COLUMN auto_match_eligible SET DEFAULT true;
