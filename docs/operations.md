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

Management pages reference embedded assets through a content-versioned URL.
These assets use `Cache-Control: public, max-age=31536000, immutable`, so a
browser can reuse them without a network request. A change to any embedded
asset changes the URL in newly rendered pages. Unversioned asset URLs remain
available for older pages and use ETags with mandatory revalidation.

## Retention and maintenance

The public API periodically removes expired challenges, rate buckets, upload
leases, and abandoned staging directories. The admin service removes expired
authentication state, reports, attachment metadata and files, and audit events.

Sweep frequency, staging retention, and the retention of reports, submission sources
and audit events live in the runtime configuration and are editable in the
management UI. Records get their expiry when accepted, and the maintenance loop
backfills it for older records.

Report and audit retention default to ten years. The keyed submission source
identifier on a report is cleared 48 hours after the report was created by default
(`submission_source_retention_hours`), independently of report retention. The
maintenance sweep does this, so an identifier can outlast the period by up to one
sweep interval. Rate-limit buckets and upload leases, which also hold the keyed
identifier, expire within hours and are removed by the public API sweep. Active source blocks keep their own identifier; lifted or expired blocks
are removed after the source retention period.

## Database statistics

Index and table usage is tracked by PostgreSQL by default, in `pg_stat_user_indexes`
and `pg_stat_user_tables`. Function calls are not: `track_functions` is `none`, so
there is no way to tell whether a function is still used. Before removing a
function or trigger, a superuser can turn tracking on for the database and read
`pg_stat_user_functions` after a representative period:

```sql
ALTER DATABASE ladybird_reports SET track_functions = 'pl';
SELECT funcname, calls FROM pg_stat_user_functions ORDER BY calls;
```

The setting applies to new connections, so restart the services after changing it.
It is not part of the migrations because only superusers can set it.

Judge indexes on a database of realistic size. With a few hundred reports
PostgreSQL reads tables sequentially and a useful index shows no scans; at tens of
thousands of reports the list, expiry, issue and GitHub lookups all use the
indexes the schema defines.
