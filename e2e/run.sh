#!/usr/bin/env bash
# Builds the desktop app with the production UI embedded and the IMAP/SMTP test server, then
# runs the end-to-end suite against them (Linux, headless via Xvfb + tauri-driver). Nothing
# goes to the internet: every account talks to the test server on this computer.
set -euo pipefail
cd "$(dirname "$0")/.."
npm --prefix ui run build
cargo build -p arcalo-mail --features custom-protocol -p mail-testserver
cd e2e
rm -f screenshots/FAIL-*
[ -d node_modules ] || npm ci
ARCALO_MAIL_APP="$PWD/../target/debug/arcalo-mail" node --test --test-concurrency=1 --test-reporter=spec tests/*.test.js
