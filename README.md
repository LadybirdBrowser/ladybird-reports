# Ladybird Reports

Ladybird Reports accepts anonymous diagnostic reports with flexible typed fields and
attachments. It provides a private triage and issue-management interface. The Rust
service uses PostgreSQL and a persistent attachment directory.

The repository is split into two long-running processes:

- `ladybird-reports-public-api` accepts untrusted submissions with a restricted
  PostgreSQL role.
- `ladybird-reports-admin` migrates the database and serves the GitHub-authenticated
  management UI with the administrative PostgreSQL role.

Both processes are built into the same container image. See
[architecture.md](docs/architecture.md), [protocol-v1.md](docs/protocol-v1.md), and
[threat-model.md](docs/threat-model.md) for the design details.

## First deployment

Create a private GitHub App owned by the `LadybirdBrowser` organization. Configure
its user authorization as follows:

- Homepage URL: `https://<admin-host>/`
- Callback URL: `https://<admin-host>/auth/callback`
- Request user authorization during installation: disabled
- Device flow: disabled
- Webhooks: inactive
- Repository permission `Issues`: read and write
- Organization permission `Members`: read-only
- Installation availability: only the account that owns the app

Install the app on the organization and grant it access only to the repository used
for issue tracking. The default runtime configuration uses
`LadybirdBrowser/ladybird`. The app does not need a private key because the server
uses user access tokens rather than installation access tokens.

The production callback URL is:

```text
https://reports.app.ladybird.org/auth/callback
```

Copy the GitHub App client ID, generate a client secret, and provide both to the
admin service. Start the admin container with `ADMIN_DATABASE_URL`, the GitHub App
credentials, `SESSION_ENCRYPTION_KEY`, and the persistent attachment volume. Leave
`REPORTING_DATABASE_URL` unset. The admin process will:

1. create or migrate the schema;
2. create a random PostgreSQL login with only the ingestion grants;
3. encrypt its generated connection URL in PostgreSQL; and
4. show that URL only after an authorized account signs in.

Add the displayed URL as `REPORTING_DATABASE_URL` to the admin service and restart
it manually. Startup verifies and reapplies the restricted grants, then removes the
temporary encrypted credential. Configure a second Coolify service from the same
image, override its command with `/usr/local/bin/ladybird-reports-public-api`, and
give that service only `REPORTING_DATABASE_URL`, `POW_HMAC_KEY`,
`CLIENT_ADDRESS_HMAC_KEY`, and the shared attachment volume.

Generate each 32-byte key with:

```sh
openssl rand -base64 32
```

The image contains no deployment secrets. PostgreSQL connection URLs and keys must
be Coolify secrets. Runtime limits and application configuration live in the
`runtime_configuration` row and can be edited in the management UI.

## Environment

| Variable | Process | Purpose |
| --- | --- | --- |
| `ADMIN_DATABASE_URL` | admin, migrate | Full PostgreSQL URL; the role must be able to migrate and create roles. |
| `REPORTING_DATABASE_URL` | public API, then admin | Generated restricted PostgreSQL URL. The admin uses it only to verify grants. |
| `SESSION_ENCRYPTION_KEY` | admin | Base64-encoded 32-byte key for GitHub tokens and pending setup credentials. |
| `GITHUB_CLIENT_ID` | admin | GitHub App client ID. |
| `GITHUB_CLIENT_SECRET` | admin | GitHub App client secret used for the user authorization flow. |
| `POW_HMAC_KEY` | public API | Base64-encoded 32-byte challenge-signing key. |
| `CLIENT_ADDRESS_HMAC_KEY` | public API | Base64-encoded 32-byte key for pseudonymous rate limits and source blocks. Keep it stable so existing blocks continue to match. |
| `ATTACHMENT_ROOT` | both | Shared persistent directory; defaults to `./data/attachments` locally and `/data/attachments` in the image. |
| `ADMIN_LISTEN_ADDRESS` | admin | Defaults to `0.0.0.0:3000`. |
| `PUBLIC_LISTEN_ADDRESS` | public API | Defaults to `0.0.0.0:3001`. |
| `RUST_LOG` | both | `tracing` filter, such as `ladybird_reports=debug,tower_http=info`. |

## Tests

Install Rust 1.88 or newer, Node.js, and PostgreSQL 17. Then run the complete local
suite:

```sh
./scripts/test-local.sh
```

The script installs locked Node dependencies and Chromium, creates an isolated
temporary PostgreSQL cluster on a free loopback port, runs Rust unit and integration
tests, launches the real admin binary, and runs Playwright through the management
workflow. It stops and deletes the temporary cluster when finished.

To use an existing disposable database instead:

```sh
ADMIN_DATABASE_URL=postgresql://... \
TEST_ADMIN_DATABASE_URL=postgresql://... \
SESSION_ENCRYPTION_KEY=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA= \
GITHUB_CLIENT_ID=test \
GITHUB_CLIENT_SECRET=test \
npm run test:browser
```

GitHub Actions runs formatting, Clippy, Rust tests, and browser tests against separate
PostgreSQL 17 service containers. For pushes to `master`, a dependent deployment job
sends the signed push event to Coolify only after both test jobs pass. Coolify then
builds the repository Dockerfile from that exact commit. The direct GitHub repository
webhook must remain disabled so unverified pushes cannot bypass this gate.

## Logs and health checks

Both services write compact, line-oriented text logs to container output. Records
include a timestamp, level, event name, and relevant fields such as request ID, method,
path, status, and latency. Lifecycle events cover migrations, credential setup, report
acceptance, attachment recovery, and GitHub linking or creation. Report contents,
attachment contents, submitted URLs, raw client addresses, access tokens, and database
passwords are never logged.

The request ID is also returned as `X-Request-Id`, which makes a failed HTTP request
easy to match with its container logs.

Dynamic management pages use `Cache-Control: private, no-store`, and public API
responses use `Cache-Control: no-store`. Both services add a restrictive content
security policy and browser security headers to every response. The embedded admin
stylesheet and JavaScript use content-derived ETags with mandatory revalidation.
Browsers can reuse unchanged assets with a `304 Not Modified` response, while a new
container image automatically changes the ETag whenever an embedded asset changes.

- `GET /health/live` confirms that the process can answer HTTP.
- `GET /health/ready` checks PostgreSQL and, for the public API, attachment storage.

## Container commands

The image defaults to the admin service. These additional executables are available:

```text
/usr/local/bin/ladybird-reports-public-api
/usr/local/bin/ladybird-reports-migrate
```
