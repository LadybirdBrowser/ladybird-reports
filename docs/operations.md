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
authentication state, reports, attachment metadata, and attachment files.

Sweep frequency, staging retention, and report retention live in the database-backed
runtime configuration and are editable in the management UI. New reports and
attachments receive expiry timestamps when accepted. The maintenance loop also
backfills expiry timestamps for records created before retention was enabled.

The default report retention is ten years. The keyed submission source identifier
on a report expires after 30 days by default, independently of report retention.
Both periods are configurable in the management UI. Active source blocks retain
their own keyed identifier; lifted or expired blocks are removed after the source
retention period. Review retention settings as operational
and privacy requirements become clearer.

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
