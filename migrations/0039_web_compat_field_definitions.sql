-- The fields that Ladybird sends with a web compatibility report and that had no definition yet, so they showed up
-- as additional fields. A definition that was added by hand in the meantime stays as it is.

INSERT INTO public.field_definitions (key, label, kind, "position") VALUES
    ('user_agent', 'User agent', 'text', 65),
    ('viewport_size', 'Viewport size', 'text', 70),
    ('device_pixel_ratio', 'Device pixel ratio', 'number', 71),
    ('zoom_level', 'Zoom level', 'number', 72)
ON CONFLICT (key) DO NOTHING;
