# Measure startup time, idle memory and idle CPU of mp3palace (whole process
# tree: engine + WebView2 browser/renderers/GPU).
#   powershell -File scripts/measure.ps1 [-Exe path] [-IdleSeconds 30] [-Runs 3]
param(
  [string]$Exe = "$PSScriptRoot\..\src-tauri\target\release\mp3palace.exe",
  [int]$IdleSeconds = 30,
  [int]$Runs = 3
)
$ErrorActionPreference = "Stop"
$startupFile = Join-Path $env:LOCALAPPDATA "mp3palace\last_startup_ms.txt"

function Stop-App {
  Get-Process mp3palace -ErrorAction SilentlyContinue | Stop-Process -Force
  Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" |
    Where-Object { $_.CommandLine -like '*webview-exe-name=mp3palace.exe*' } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  Start-Sleep -Milliseconds 1500
}

function Get-Tree([int]$root) {
  $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name
  $ids = New-Object System.Collections.Generic.List[int]
  $ids.Add($root)
  $changed = $true
  while ($changed) {
    $changed = $false
    foreach ($p in $all) {
      if ($ids.Contains([int]$p.ParentProcessId) -and -not $ids.Contains([int]$p.ProcessId)) { $ids.Add([int]$p.ProcessId); $changed = $true }
    }
  }
  $ids
}

$results = @()
for ($i = 1; $i -le $Runs; $i++) {
  Stop-App
  Remove-Item $startupFile -ErrorAction SilentlyContinue
  $proc = Start-Process -FilePath $Exe -PassThru
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while (-not (Test-Path $startupFile) -and $sw.ElapsedMilliseconds -lt 15000) { Start-Sleep -Milliseconds 25 }
  $startup = if (Test-Path $startupFile) { (Get-Content $startupFile -Raw).Trim() } else { "timeout" }
  Start-Sleep -Seconds $IdleSeconds
  $tree = Get-Tree $proc.Id
  $procs = $tree | ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue } | Where-Object { $_ }
  $ws = ($procs | Measure-Object WorkingSet64 -Sum).Sum / 1MB
  $priv = ($procs | Measure-Object PrivateMemorySize64 -Sum).Sum / 1MB
  $engine = (Get-Process -Id $proc.Id).WorkingSet64 / 1MB
  $cpu0ms = ($procs | ForEach-Object { $_.TotalProcessorTime.TotalMilliseconds } | Measure-Object -Sum).Sum
  $t0 = Get-Date
  Start-Sleep -Seconds 10
  $procs2 = $tree | ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue } | Where-Object { $_ }
  $cpu1 = ($procs2 | ForEach-Object { $_.TotalProcessorTime.TotalMilliseconds } | Measure-Object -Sum).Sum
  $elapsed = ((Get-Date) - $t0).TotalMilliseconds
  $cpuPct = ($cpu1 - $cpu0ms) / ($elapsed * [Environment]::ProcessorCount) * 100
  $r = [pscustomobject]@{
    run = $i; startup_ms = $startup; processes = $procs.Count
    working_set_mb = [math]::Round($ws, 1); private_mb = [math]::Round($priv, 1)
    engine_ws_mb = [math]::Round($engine, 1); idle_cpu_pct = [math]::Round($cpuPct, 2)
  }
  $results += $r
  $r | Format-List | Out-String | Write-Host
}
Stop-App
$results | Format-Table -AutoSize | Out-String -Width 200
