#!/bin/sh
set -eu

project_root=$(cd -- "$(dirname "$0")/.." >/dev/null 2>&1 && pwd)
cd "$project_root"

export SESSION_ENCRYPTION_KEY="${SESSION_ENCRYPTION_KEY:-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=}"
export GITHUB_CLIENT_ID="${GITHUB_CLIENT_ID:-browser-test-client}"
export GITHUB_CLIENT_SECRET="${GITHUB_CLIENT_SECRET:-browser-test-secret}"
export ATTACHMENT_ROOT="${ATTACHMENT_ROOT:-$project_root/target/browser-test-attachments}"

npm ci
npx playwright install chromium

exec "$project_root/scripts/with-test-postgres.sh" \
    "$project_root/scripts/test-in-database.sh"
