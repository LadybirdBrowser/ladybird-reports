#!/bin/sh
set -eu

node tests/browser/fake-github.mjs &
github_pid=$!

cargo run --quiet --bin admin &
admin_pid=$!

cleanup() {
    kill "$admin_pid" 2>/dev/null || true
    kill "$github_pid" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

wait "$admin_pid"
