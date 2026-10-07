-- Fields of these types are shown as links: a URL behind a warning, since it is
-- untrusted input, and a commit ID to the commit in the upstream repository.

ALTER TABLE public.field_definitions DROP CONSTRAINT field_definitions_kind_check;
ALTER TABLE public.field_definitions ADD CONSTRAINT field_definitions_kind_check
    CHECK (kind = ANY (ARRAY['text', 'multiline', 'stack_trace', 'url', 'commit_id', 'number', 'boolean', 'attachment']));

ALTER TABLE public.report_fields DROP CONSTRAINT report_fields_kind_check;
ALTER TABLE public.report_fields ADD CONSTRAINT report_fields_kind_check
    CHECK (kind = ANY (ARRAY['text', 'multiline', 'stack_trace', 'url', 'commit_id', 'number', 'boolean', 'attachment']));

UPDATE public.field_definitions SET kind = 'url' WHERE key = 'url' AND kind = 'text';
UPDATE public.field_definitions SET kind = 'commit_id' WHERE key = 'git_commit' AND kind = 'text';
