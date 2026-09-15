# Development and testing

Ladybird Reports requires Rust 1.88 or newer, Node.js, and PostgreSQL 17.

## Complete local suite

Run:

```sh
./scripts/test-local.sh
```

The script installs locked Node dependencies and Chromium, creates an isolated
temporary PostgreSQL cluster on a free loopback port, runs Rust unit and integration
tests, launches the real admin binary, and runs Playwright through the management
workflow. It stops and deletes the temporary cluster when finished.

## Existing test database

To run the browser tests against an existing disposable database:

```sh
ADMIN_DATABASE_URL=postgresql://... \
TEST_ADMIN_DATABASE_URL=postgresql://... \
SESSION_ENCRYPTION_KEY=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA= \
GITHUB_CLIENT_ID=test \
GITHUB_CLIENT_SECRET=test \
npm run test:browser
```

The browser fixture truncates application tables. Never point
`TEST_ADMIN_DATABASE_URL` at a database containing data you need to keep.

## Continuous integration

GitHub Actions runs these checks with separate PostgreSQL 17 service containers:

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --all-targets`
- the Playwright management workflow

The production image is built only after both test jobs pass.
