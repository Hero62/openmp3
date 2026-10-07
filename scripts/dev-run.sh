#!/usr/bin/env bash
# Start the dev build in the background with CDP enabled (port 9222).
# Stop it with scripts/dev-stop.sh. Log: /tmp/mp3palace-dev.log
cd "$(dirname "$0")/../src-tauri"
export WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"
./target/debug/mp3palace.exe >/tmp/mp3palace-dev.log 2>&1 &
echo $! > /tmp/mp3palace-dev.pid
