# Architecture

Ladybird Reports is distributed as one image with three executable modes:

- `public_api` accepts anonymous reports.
- `admin` serves the authenticated management interface.
- `migrate` applies schema migrations explicitly; normal admin startup also migrates.

The public and admin processes are separate deployments. The public process only
receives the restricted ingestion database credential and proof-of-work key. The
admin process receives the full database credential and GitHub secrets. Keeping
both credentials out of one address space is a deliberate security boundary.

## Layers

The domain layer contains validated values and state transitions. It has no HTTP,
database, template, or filesystem dependencies beyond serialization types.

The application layer coordinates use cases such as accepting a report or assigning
reports to an issue. It depends on narrow infrastructure interfaces.

The infrastructure layer implements PostgreSQL access, attachment storage, and the
GitHub API. SQL and filesystem recovery behavior live here.

The admin service authenticates with GitHub App user access tokens. The app requests
only read-only organization membership and read-write issue permissions. Every
GitHub API action is therefore limited by both the app's permissions and the signed-in
user's permissions. The login flow separately verifies active membership in the
configured authorization team.

GitHub is authoritative for an issue's open or closed state. The admin service
accepts signed GitHub App issue webhooks and refreshes an issue from GitHub when
it is opened in the management UI. Reports owns assignment and visibility:
closing or deleting a GitHub issue never removes its linked reports. A missing
or moved GitHub issue stays visible as needing attention until a maintainer
links a replacement. Old GitHub links remain searchable through aliases.

The web layer translates requests into application calls. Public handlers return
JSON. Admin handlers construct typed view models rendered by Askama templates.

## Discord notifications

When a report becomes ready, a database trigger adds it to a transactional outbox in
the same transaction. The admin process claims the oldest pending notification and
sends a bounded report summary through the configured Discord webhook. The summary
includes an excerpt of the native stack when one is available and links to the full
report in the management interface. URLs, source addresses, and other report fields
are not copied into the message by default.

Only one delivery may be in progress across all admin replicas. An unsuccessful
request pauses the complete queue and retries its oldest entry with exponential
backoff. Discord rate-limit delays are honored. A notification is marked delivered
only after Discord confirms the message; a process or database failure after that
confirmation can therefore produce a duplicate during recovery. This at-least-once
behavior avoids silently losing a notification.

The webhook URL lives in the database-backed runtime configuration. The reporting
database role reads a projection that excludes Discord configuration, so the public
API process cannot retrieve the credential.

## Identifiers

Every identifier in the protocol, application, and database is UUIDv7. The server
generates report, challenge, upload, issue, attachment-storage, GitHub-attempt, and
request identifiers. Clients generate submission and attachment identifiers. Rust
types reject other UUID versions at input boundaries, and database checks enforce the
same rule for privileged writes.

## Attachments

Attachments are written beneath `staging/<upload-id>` and validated before database
acceptance. The database transaction creates a report in `pending_files` and consumes
the challenge. The application then atomically renames the staging directory to
`reports/<report-id>` and marks the report `ready`.

The public API can recover an interrupted finalization. Admin pages only expose ready
reports. Dropping an interrupted request schedules immediate staging cleanup. A
periodic public API sweep is the durable fallback: it removes expired challenges,
rate buckets, upload leases, and staging directories that are both old enough and
unreferenced by a report.

## Retention

New reports and attachments receive an expiry derived from the database-backed runtime
configuration. The default is ten years. The admin maintenance loop backfills that
expiry on older records, marks expired records for deletion, removes their attachment
directories, and then deletes the database rows. It also removes expired sessions and
OAuth state. Sweep frequency, staging retention, and report retention are editable in
the management settings and constrained by compiled safety ranges.

## Operations

Both HTTP processes emit compact, line-oriented text to container output. Every request
is assigned a UUID that is included in its response and in a completion event with
the method, path, status, and latency. Domain lifecycle events use stable `event`
values. Sensitive report values and secrets are excluded so container logs remain
safe enough for routine diagnostics and forwarding.
