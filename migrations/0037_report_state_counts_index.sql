-- The report list counts reports per state on every load. A narrow index lets
-- that count be answered from the index alone.

CREATE INDEX reports_state_counts ON public.reports USING btree (state)
    WHERE (storage_state = 'ready');
