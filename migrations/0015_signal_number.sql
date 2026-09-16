INSERT INTO field_definitions (key, label, kind, position)
VALUES ('signal_number', 'Signal number', 'number', 11)
ON CONFLICT (key) DO NOTHING;
