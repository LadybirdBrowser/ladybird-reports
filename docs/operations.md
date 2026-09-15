# Operations

## Logs

Both services write compact, line-oriented text logs to stdout and stderr. Records
include a timestamp, level, event name, and relevant fields such as request ID,
method, path, status, and latency.

Lifecycle events cover migrations, credential setup, report acceptance, attachment
recovery, maintenance sweeps, and GitHub linking or creation. Report contents,
attachment contents, submitted URLs, raw client addresses, access tokens, and
database passwords are excluded.

Every HTTP response includes `X-Request-Id`. Use it to find the matching request in
the container logs.

## Health checks

- `GET /health/live` confirms that the process can answer HTTP.
- `GET /health/ready` checks PostgreSQL and, for the public API, attachment storage.

Configure the container platform to use `/health/ready` for both services.

## Response security and caching

Dynamic management pages use `Cache-Control: private, no-store`. Public API
responses use `Cache-Control: no-store`. Both services add a restrictive content
security policy and browser security headers to every response.

The embedded admin stylesheet and JavaScript use content-derived ETags with
mandatory revalidation. Browsers can reuse unchanged assets with a `304 Not
Modified` response. Changing an embedded asset changes its ETag when a new image is
deployed.

## Retention and maintenance

The public API periodically removes expired challenges, rate buckets, upload
leases, and abandoned staging directories. The admin service removes expired
authentication state, reports, attachment metadata, and attachment files.

Sweep frequency, staging retention, and report retention live in the database-backed
runtime configuration and are editable in the management UI. New reports and
attachments receive expiry timestamps when accepted. The maintenance loop also
backfills expiry timestamps for records created before retention was enabled.

The default report retention is ten years. Review that setting as the operational
and privacy requirements become clearer.
