#!/usr/bin/env bash
taskkill //F //IM mp3palace.exe >/dev/null 2>&1
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='msedgewebview2.exe'\" | ? { \$_.CommandLine -like '*webview-exe-name=mp3palace.exe*' } | % { Stop-Process -Id \$_.ProcessId -Force -EA SilentlyContinue }" 2>/dev/null
sleep 1
