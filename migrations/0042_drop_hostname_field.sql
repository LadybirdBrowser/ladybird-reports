-- The page URL already contains the website hostname, and clients no longer send it separately. Reports that still
-- carry a hostname show it as an additional field.

DELETE FROM public.field_definitions WHERE key = 'hostname';
