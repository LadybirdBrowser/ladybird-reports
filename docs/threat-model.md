# Threat model

The anonymous API is hostile input. It must remain bounded in request size, decoded image size, field count, processing
time, storage use, request rate, and concurrent uploads. PNG validation applies both a pixel ceiling and a decoded-byte
ceiling before allocating its frame buffer. A weighted semaphore caps aggregate decoder reservations at 128 MiB per
public API process. Submitted content is never written to logs or interpreted as HTML.

Proof of work raises the cost of bulk submissions but does not establish authenticity. IP rate limits and global
capacity limits remain necessary.

The server HMACs normalized client addresses for rate-limit buckets and source blocks. Reports retain that keyed
identifier for the configured submission source retention period (48 hours by default), so an administrator can block
abusive sources from the report action panel. The maintenance sweep clears expired identifiers from reports. Active
blocks keep their own keyed identifiers. Lifted or expired blocks are removed after the configured retention period. Raw
source addresses are not stored on reports or written to application logs. Rotating `CLIENT_ADDRESS_HMAC_KEY`
intentionally breaks matching against earlier blocks, so the key must remain stable during normal operation.

The ingestion database role cannot read report bodies, attachments, sessions, audit events, or GitHub tokens. It can
execute narrowly scoped functions for challenges, rate limits, upload leases, idempotency checks, and report acceptance.

Management access requires GitHub authentication and an authorized account. The GitHub App uses a user access token with
read-only organization membership and read-write issue permissions; it has no source-code write permission.
State-changing forms require a CSRF token. GitHub issue creation publishes the entered title and description. The
initial proposal includes selected report context, the raw stack trace, and the page URL, while omitting unknown fields
and attachments. A report's screenshot is the exception: the draft holds a placeholder that is uploaded to GitHub, as
the upload account, when the issue is created, and a maintainer can remove it. That account's token can only edit issues
in the tracking repository. Maintainers review the public draft before creating the issue.
Images in an issue's description that were uploaded to GitHub are fetched by the admin service from a fixed GitHub
address, by identifier, and shown only when GitHub answers with a raster image of at most 10 MiB. The request carries no
credentials, and the identifier is validated, so a description cannot make the service fetch an arbitrary address. GitHub's
redirect is followed only to a named `amazonaws.com` host over HTTPS, never to an IP address or another site.

A maintainer can share a report as YAML through a link that works for 24 hours without signing in, for agents that
work on it. The link carries what the public GitHub draft leaves out, such as the whole page address and unknown fields,
and the attachments, so creating one is a deliberate act that is audited. The token is 256 bits of randomness and only
its hash is stored, so a link is shown once and cannot be listed or revoked; it ends after 24 hours or with its report.
The token is masked in request logs, and an unknown or expired link looks the same as any other. The route is the only
one of the admin service that needs no session. It answers with `text/plain` or the attachment, and sets no cookie. The
document marks everything from the reporter as untrusted, since an agent reading it could otherwise be steered by what
an anonymous client wrote.

Reports, attachments, and audit events expire after the configured retention period; short-lived unauthenticated and
authentication records expire individually. Maintenance deletes them along with attachment files.

Secrets are supplied at runtime by the deployment platform. They are absent from the source tree, container build
arguments, image environment, and logs.
