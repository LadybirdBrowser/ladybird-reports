-- A maintainer can share a report through a link that works for 24 hours without signing in, such as for an agent that
-- works on it. Only a hash of the link's token is stored, so the link cannot be recovered afterwards.

CREATE TABLE public.report_shares (
    id uuid PRIMARY KEY,
    report_id uuid NOT NULL REFERENCES public.reports(id) ON DELETE CASCADE,
    token_hash text NOT NULL UNIQUE,
    created_by bigint NOT NULL,
    created_at timestamp with time zone NOT NULL DEFAULT now(),
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    use_count bigint NOT NULL DEFAULT 0,
    last_used_at timestamp with time zone,
    CONSTRAINT report_shares_id_check CHECK (public.is_uuid_v7(id)),
    CONSTRAINT report_shares_token_hash_check CHECK (token_hash ~ '^[0-9a-f]{64}$'),
    CONSTRAINT report_shares_expiry_check CHECK (expires_at > created_at)
);

CREATE INDEX report_shares_report_id_idx ON public.report_shares (report_id, created_at DESC);
CREATE INDEX report_shares_expires_at_idx ON public.report_shares (expires_at);
