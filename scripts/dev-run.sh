#!/usr/bin/env bash
# Start the dev build in the background with CDP enabled (port 9222).
# Stop it with scripts/dev-stop.sh. Log: /tmp/openmp3-dev.log
cd "$(dirname "$0")/../src-tauri"
export WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"
./target/debug/openmp3.exe >/tmp/openmp3-dev.log 2>&1 &
echo $! > /tmp/openmp3-dev.pid
