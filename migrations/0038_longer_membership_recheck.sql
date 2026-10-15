-- Sessions are verified in the background before their membership check is due, so the check can be less frequent. Only
-- the previous default changes; a value an operator chose is left alone.

UPDATE public.runtime_configuration
SET value = jsonb_set(value, '{membership_recheck_seconds}', '1800')
WHERE singleton = true AND value -> 'membership_recheck_seconds' = '600'::jsonb;
