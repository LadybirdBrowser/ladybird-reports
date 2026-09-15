CREATE UNIQUE INDEX one_active_issue_per_github_issue
    ON issues (github_number)
    WHERE github_number IS NOT NULL AND merged_into IS NULL;
