# Deployment

The production image contains the admin service, public API, and migration
utility. It contains no deployment secrets. Store PostgreSQL connection URLs,
cryptographic keys, and GitHub credentials as deployment secrets.

## GitHub App

Create a private GitHub App owned by the `LadybirdBrowser` organization. Configure
its user authorization as follows:

- Homepage URL: `https://<admin-host>/`
- Callback URL: `https://<admin-host>/auth/callback`
- Request user authorization during installation: disabled
- Device flow: disabled
- Webhooks: active; subscribe to the `Issues` event
- Repository permission `Issues`: read and write
- Organization permission `Members`: read-only
- Organization permission `Issue Fields`: read-only
- Installation availability: only the account that owns the app

Install the app on the organization and grant it access only to the repository used
for issue tracking. The default runtime configuration uses
`LadybirdBrowser/ladybird`. The app does not need a private key because the server
uses user access tokens rather than installation access tokens.

The production callback URL is:

```text
https://reports.app.ladybird.org/auth/callback
```

Copy the GitHub App client ID and generate a client secret. Provide both to the
admin service.

Set the GitHub App webhook URL to `https://<admin-host>/webhooks/github`.
Generate a random webhook secret of at least 32 characters, configure it in
GitHub, and provide it to the admin service as `GITHUB_WEBHOOK_SECRET`. The
admin service verifies each webhook signature before processing it. Until this
secret is configured, issue state is refreshed when a maintainer opens an
issue, but GitHub changes will not appear immediately in the issue list.

To show a backlink in the GitHub issue sidebar, create an organization issue
field such as `Reports issue`. Use the **Text** type and **Organization only**
visibility, then pin it to the issue types you use (including issues without a
type). Put its numeric field ID in the runtime setting
`github_reports_issue_field_id`. The admin service checks the field's type and
visibility before writing an issue URL; it retries failed links when the issue
is viewed. GitHub's organization-only visibility also includes repository
collaborators, so it is broader than the Reports maintainer login policy.

## Database bootstrap

Start the admin container with `ADMIN_DATABASE_URL`, the GitHub App credentials,
`SESSION_ENCRYPTION_KEY`, and the persistent attachment volume. Leave
`REPORTING_DATABASE_URL` unset. The admin process will:

1. create or migrate the schema;
2. create a random PostgreSQL login with only the ingestion grants;
3. encrypt its generated connection URL in PostgreSQL; and
4. show that URL only after an authorized account signs in.

Add the displayed URL as `REPORTING_DATABASE_URL` to the admin service and restart
it manually. Startup verifies and reapplies the restricted grants, then removes the
temporary encrypted credential.

## Service topology

Run two services from the same
`ghcr.io/ladybirdbrowser/ladybird-reports:master` image and a shared persistent
attachment directory.

The admin resource uses the image's default command and listens on port 3000. Give
it the administrative database URL, restricted database URL, GitHub credentials,
session encryption key, and attachment volume.

The public API resource must override the command with:

```text
/usr/local/bin/ladybird-reports-public-api
```

It listens on port 3001. Give it only the restricted database URL, proof-of-work
key, client-address HMAC key, and attachment volume. Do not provide the public API
with the administrative database URL, GitHub credentials, or session encryption
key.

Mount the same host directory at `/data/attachments` in both resources. Keep the
directory outside ephemeral container storage; for example:

```text
/data/apps/ladybird-reports/attachments
```

## Image delivery

GitHub Actions runs formatting, Clippy, Rust tests, and browser tests before it
builds an image. A successful push to `master` publishes immutable commit and
moving `master` tags to GitHub Container Registry. The final workflow job sends
signed deployment events to the configured deployment targets. The container
platform pulls the published image and does not build application source.

The image build exports its BuildKit cache to GitHub Actions. Its `cargo-chef`
dependency layer changes only when the Rust package manifests change, so ordinary
source and template updates reuse compiled release dependencies across CI runs.

The image build embeds a display version in the form `YYYY.MM.DD-aaaaaaaa`, using
the UTC build date and the first eight characters of the Git commit identifier.

## Environment

| Variable | Process | Purpose |
| --- | --- | --- |
| `ADMIN_DATABASE_URL` | admin, migrate | Full PostgreSQL URL; the role must be able to migrate and create roles. |
| `REPORTING_DATABASE_URL` | public API, then admin | Generated restricted PostgreSQL URL. The admin uses it only to verify grants. |
| `SESSION_ENCRYPTION_KEY` | admin | Base64-encoded 32-byte key for GitHub tokens and pending setup credentials. |
| `GITHUB_CLIENT_ID` | admin | GitHub App client ID. |
| `GITHUB_CLIENT_SECRET` | admin | GitHub App client secret used for the user authorization flow. |
| `GITHUB_WEBHOOK_SECRET` | admin | Secret for verifying signed GitHub issue events. Set the same value in the GitHub App. |
| `POW_HMAC_KEY` | public API | Base64-encoded 32-byte challenge-signing key. |
| `CLIENT_ADDRESS_HMAC_KEY` | public API | Base64-encoded 32-byte key for pseudonymous rate limits and source blocks. Keep it stable so existing blocks continue to match. |
| `ATTACHMENT_ROOT` | both | Shared persistent directory; defaults to `./data/attachments` locally and `/data/attachments` in the image. |
| `ADMIN_LISTEN_ADDRESS` | admin | Defaults to `0.0.0.0:3000`. |
| `PUBLIC_LISTEN_ADDRESS` | public API | Defaults to `0.0.0.0:3001`. |
| `RUST_LOG` | both | `tracing` filter, such as `ladybird_reports=debug,tower_http=info`. |

Generate each 32-byte key with:

```sh
openssl rand -base64 32
```

Runtime limits and application configuration live in the
`runtime_configuration` database row and can be edited in the management UI.

## Container commands

The image defaults to the admin service. It also contains:

```text
/usr/local/bin/ladybird-reports-public-api
/usr/local/bin/ladybird-reports-migrate
```
