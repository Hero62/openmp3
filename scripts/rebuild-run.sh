#!/usr/bin/env bash
# Stop the running dev app, rebuild, start it again with CDP enabled.
here="$(dirname "$0")"
"$here/dev-stop.sh"
(cd "$here/../src-tauri" && cargo build 2>&1 | grep -E "^(error|warning: unused)" -A8 | head -40)
"$here/dev-run.sh"
