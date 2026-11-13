-- Ladybird reports what it decided about each of its platform features, which is shown as the tree of features
-- that its about:support page shows, along with the GPU the decisions were made for.

ALTER TABLE public.field_definitions DROP CONSTRAINT field_definitions_kind_check;
ALTER TABLE public.field_definitions ADD CONSTRAINT field_definitions_kind_check
    CHECK (kind = ANY (ARRAY['text', 'multiline', 'stack_trace', 'url', 'commit_id', 'number', 'boolean', 'attachment', 'platform_features']));

ALTER TABLE public.report_fields DROP CONSTRAINT report_fields_kind_check;
ALTER TABLE public.report_fields ADD CONSTRAINT report_fields_kind_check
    CHECK (kind = ANY (ARRAY['text', 'multiline', 'stack_trace', 'url', 'commit_id', 'number', 'boolean', 'attachment', 'platform_features']));

INSERT INTO public.field_definitions (key, label, kind, "position") VALUES
    ('gpu_device', 'GPU', 'text', 36),
    ('gpu_driver', 'GPU driver', 'text', 37),
    ('platform_features', 'Platform features', 'platform_features', 45)
ON CONFLICT (key) DO NOTHING;
