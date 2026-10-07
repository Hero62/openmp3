#!/usr/bin/env bash
# Run the dev build for N seconds (default 8), capture stderr/stdout, then kill
# the whole process tree (including WebView2 helpers) so the next run is clean.
secs=${1:-8}; shift
exe="$(dirname "$0")/../src-tauri/target/debug/openmp3.exe"
log=${SMOKE_LOG:-/tmp/openmp3-smoke.log}
"$exe" "$@" >"$log" 2>&1 &
pid=$!
sleep "$secs"
winpid=$(cat /proc/$pid/winpid 2>/dev/null)
[ -n "$winpid" ] && taskkill //F //T //PID "$winpid" >/dev/null 2>&1
kill $pid 2>/dev/null
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='msedgewebview2.exe'\" | ? { \$_.CommandLine -like '*webview-exe-name=openmp3.exe*' } | % { Stop-Process -Id \$_.ProcessId -Force -EA SilentlyContinue }" 2>/dev/null
sleep 1
cat "$log"
