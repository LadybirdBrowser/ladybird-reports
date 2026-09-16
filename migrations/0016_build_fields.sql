UPDATE field_definitions
SET label = 'Stack trace'
WHERE key = 'stack' AND label = 'Native stack';

INSERT INTO field_definitions (key, label, kind, position)
VALUES
    ('git_commit', 'Git commit', 'text', 32),
    ('build_configuration', 'Build configuration', 'text', 33),
    ('cpp_compiler', 'C++ compiler', 'text', 34)
ON CONFLICT (key) DO NOTHING;
