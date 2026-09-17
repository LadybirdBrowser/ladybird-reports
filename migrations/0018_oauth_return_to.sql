ALTER TABLE oauth_states
    ADD COLUMN return_to text NOT NULL DEFAULT '/'
        CHECK (length(return_to) <= 2048 AND return_to LIKE '/%' AND return_to NOT LIKE '//%');
