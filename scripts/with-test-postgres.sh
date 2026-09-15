#!/bin/sh
set -eu

if [ -n "${ADMIN_DATABASE_URL:-}" ]; then
    export TEST_ADMIN_DATABASE_URL="${TEST_ADMIN_DATABASE_URL:-$ADMIN_DATABASE_URL}"
    exec "$@"
fi

if command -v postgres >/dev/null 2>&1; then
    postgres_binary=$(command -v postgres)
elif [ -x /opt/homebrew/opt/postgresql@17/bin/postgres ]; then
    postgres_binary=/opt/homebrew/opt/postgresql@17/bin/postgres
else
    echo "PostgreSQL 17 is required. Set ADMIN_DATABASE_URL or install postgresql@17." >&2
    exit 1
fi

postgres_bin=$(dirname "$postgres_binary")
test_root=$(mktemp -d "${TMPDIR:-/tmp}/ladybird-reports-tests.XXXXXX")
postgres_data="$test_root/postgres"
postgres_port=$(python3 -c \
    'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')

cleanup() {
    "$postgres_bin/pg_ctl" -D "$postgres_data" -m immediate stop >/dev/null 2>&1 || true
    find "$test_root" -depth -delete
}
trap cleanup EXIT INT TERM

"$postgres_bin/initdb" -D "$postgres_data" --auth=trust --no-locale >/dev/null
"$postgres_bin/pg_ctl" \
    -D "$postgres_data" \
    -o "-p $postgres_port -h 127.0.0.1" \
    -w start >/dev/null
"$postgres_bin/createdb" -h 127.0.0.1 -p "$postgres_port" ladybird_reports

export ADMIN_DATABASE_URL="postgresql://127.0.0.1:$postgres_port/ladybird_reports"
export TEST_ADMIN_DATABASE_URL="$ADMIN_DATABASE_URL"

"$@"
