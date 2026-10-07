#!/usr/bin/env bash
taskkill //F //IM openmp3.exe >/dev/null 2>&1
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='msedgewebview2.exe'\" | ? { \$_.CommandLine -like '*webview-exe-name=openmp3.exe*' } | % { Stop-Process -Id \$_.ProcessId -Force -EA SilentlyContinue }" 2>/dev/null
sleep 1
