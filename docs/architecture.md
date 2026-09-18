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
only read-only organization membership and read-write issue permissions.
Maintainer actions are limited by both the app's permissions and the signed-in
user's permissions. Duplicate lookups use a read-only installation token instead.
The login flow separately verifies active membership in the
configured authorization team.
The team is stored as `github_authorization_team` in the database-backed runtime
configuration, using an `organization/team-slug` value. Its initial value is
`LadybirdBrowser/maintainers`. Updating the team revokes current sessions; users
must sign in again and pass the new team's membership check.

The app encrypts both the GitHub access token and its rotating refresh token in
the session record. A request refreshes the access token shortly before it
expires, then extends the session expiry and cookie to 24 hours from that
request. Team membership is rechecked every 10 minutes. These intervals are
runtime settings: `token_refresh_before_seconds`, `session_lifetime_seconds`,
and `membership_recheck_seconds`. Sessions created before refresh-token storage
was introduced keep their original expiry and require a new sign-in.

Each process keeps a validated runtime configuration in memory. The admin
process updates the database and emits a PostgreSQL notification in the same
transaction; both services reload after receiving it. They also reload once a
minute and after reconnecting, because notifications are not durable. The
admin process updates its own cache immediately after a successful save.

GitHub is authoritative for an issue's title, description, and open or closed
state. The admin service accepts signed GitHub App issue webhooks and refreshes
an issue from GitHub when
it is opened in the management UI. Reports owns assignment and visibility:
closing or deleting a GitHub issue never removes its linked reports. A missing
or moved GitHub issue stays visible as needing attention until a maintainer
links a replacement. Old GitHub links remain searchable through aliases.
When a tracked GitHub issue closes as a duplicate, the webhook queues a job.
The admin worker uses a short-lived installation token to read GitHub's
`duplicateOf` relationship, creates the target Reports issue if needed, and
moves the linked reports in one database transaction. The source issue remains
resolved as history. Failed lookups retry without delaying webhook delivery.

The web layer translates requests into application calls. Public handlers return
JSON. Admin handlers construct typed view models rendered by Askama templates.

## Report titles

Report titles are generated when reports are read, including for older reports.
The first parsed stack frame supplies a shortened function name when possible;
otherwise the title uses report type and available process, platform, or version
information. The original stack text is unchanged. The same generated title is
suggested when creating a GitHub issue.

## Stack signatures

The public API stores stack trace text exactly as submitted in `report_fields`.
Older `stack` fields tagged `multiline` remain valid; newer clients can use the
`stack_trace` type. The admin process parses either form for a frame table and
derives a versioned signature from normalized function names, report kind, and
optional process and signal. Build IDs, addresses, paths, URLs, and source IPs do
not enter the signature. Unrecognized lines remain visible and the original text
is always available in the report view.

An admin background job indexes existing and new reports in bounded batches.
When the algorithm version changes, it rebuilds older signatures from their
preserved source text. The signature table is owned by the admin database role
and cascades away with its source field at retention time. Similarity candidates
are shown to maintainers on an unassigned report. A new report with an exact
signature match to one active, linked issue is assigned to that issue by the
background job during its first index. Historical and reindexed reports are not
assigned automatically. Conflicting issue matches stay in triage for a maintainer to
resolve. Similarity without an exact signature match never assigns a report.

## Discord notifications

When a report becomes ready, a database trigger adds it to a transactional outbox in
the same transaction. The public request finishes without waiting for signature
indexing or Discord. The admin process indexes stacks first and removes an outbox
entry if an exact signature links the report to an issue, or if an unlinked report
with the same signature arrived in the preceding five minutes. Reports with
conflicting issue matches still notify maintainers. The admin process only claims
stack reports after indexing, then claims the oldest eligible notification and
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
