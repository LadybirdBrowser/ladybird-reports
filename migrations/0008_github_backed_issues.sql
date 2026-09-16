DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM issues
        WHERE github_number IS NULL OR github_url IS NULL
    ) THEN
        RAISE EXCEPTION
            'all issues must be linked to GitHub before applying this migration';
    END IF;
END
$$;

DROP TABLE github_publish_attempts;

ALTER TABLE issues
    ALTER COLUMN github_number SET NOT NULL,
    ALTER COLUMN github_url SET NOT NULL;
