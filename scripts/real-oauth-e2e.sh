#!/usr/bin/env bash
set -euo pipefail

# This is intentionally a human-in-the-loop test. It uses a real App and real
# users, never a mocked token service. Run only on a disposable test App.
: "${LPC_TEST_APP_ID:?set LPC_TEST_APP_ID}"
: "${LPC_TEST_APP_SECRET:?set LPC_TEST_APP_SECRET}"
: "${LPC_HOME:?set LPC_HOME to a disposable directory}"

BIN_DIR="${LPC_BIN_DIR:-target/debug}"
CTL="$BIN_DIR/lpcctl"
SHIM="$BIN_DIR/lark-cli"

cargo build -p lpcctl -p lpc-shim
"$CTL" setup --shim "$SHIM"
printf '%s\n' "$LPC_TEST_APP_SECRET" | "$CTL" app import \
  --label "E2E App" --app-id "$LPC_TEST_APP_ID" --secret-stdin

APP_ID=$("$CTL" snapshot | python3 -c 'import json,sys; print(json.load(sys.stdin)["apps"][0]["id"])')
echo "Authorize test user A in the official browser page."
"$CTL" account login "$APP_ID"
echo "Authorize test user B in the official browser page."
"$CTL" account login "$APP_ID"

"$CTL" account list
printf '\nNow switch A -> run whoami -> switch B -> run whoami and verify the open IDs differ.\n'
