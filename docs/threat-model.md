# Threat model

The anonymous API is hostile input. It must remain bounded in request size, decoded
image size, field count, processing time, storage use, request rate, and concurrent
uploads. PNG validation applies both a pixel ceiling and a decoded-byte ceiling before
allocating its frame buffer. A weighted semaphore caps aggregate decoder reservations
at 128 MiB per public API process. Submitted content is never written to logs or
interpreted as HTML.

Proof of work raises the cost of bulk submissions but does not establish authenticity.
IP rate limits and global capacity limits remain necessary.

The server HMACs normalized client addresses for rate-limit buckets and source blocks.
Reports retain that keyed identifier so an administrator can impose an indefinite rate
limit from the report action panel. They also retain the source address for triage until
the report expires. The address is visible only in the management interface and is never
written to application logs. Rotating `CLIENT_ADDRESS_HMAC_KEY` intentionally breaks
matching against earlier blocks, so the key must remain stable during normal operation.

The ingestion database role cannot read report bodies, attachments, sessions, audit
events, or GitHub tokens. It can execute narrowly scoped functions for challenges,
rate limits, upload leases, idempotency checks, and report acceptance.

Management access requires GitHub authentication and an authorized account. The
GitHub App uses a user access token with read-only organization membership and
read-write issue permissions; it has no source-code write permission. State-changing
forms require a CSRF token. GitHub issue creation publishes the entered title and
description. The initial proposal includes selected report context and omits
URLs, stack traces, attachments, and unknown fields. Maintainers review the
public draft before creating the issue.

Reports and attachments expire after the configured long retention period. Maintenance
deletes expired database content and attachment files. Short-lived unauthenticated and
authentication records are removed after their individual expiry timestamps.

Secrets are supplied at runtime by the deployment platform. They are absent from
the source tree, container build arguments, image environment, and logs.
