#!/bin/sh
set -eu

cargo test --all-targets
npm run test:browser
