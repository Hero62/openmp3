# Measure openmp3 (whole process tree: engine + WebView2 processes).
#   powershell -ExecutionPolicy Bypass -File scripts/measure.ps1 [-Mode idle|playing|tray] [-Runs 2] [-Settle 25]
# Memory = Task Manager "Memory" column (private working set), summed over the tree.
# CPU    = % of the whole machine and % of one core, sampled over 15 s.
# "playing" starts playback through Windows' media controls (same path as media keys).
param(
  [string]$Exe = "$PSScriptRoot\..\src-tauri\target\release\openmp3.exe",
  [ValidateSet("idle", "playing", "tray")] [string]$Mode = "idle",
  [int]$Runs = 2,
  [int]$Settle = 25
)
$ErrorActionPreference = "Stop"
$startupFile = Join-Path $env:LOCALAPPDATA "openmp3\last_startup_ms.txt"
Add-Type -AssemblyName System.Runtime.WindowsRuntime
Add-Type @"
using System; using System.Runtime.InteropServices;
public class MeasureW { [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l); }
"@

function Stop-App {
  Get-Process openmp3 -ErrorAction SilentlyContinue | Stop-Process -Force
  Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" |
    Where-Object { $_.CommandLine -like '*webview-exe-name=openmp3.exe*' } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  Start-Sleep -Milliseconds 1500
}

function Get-TreeInfo([int]$root) {
  $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, CommandLine
  $ids = New-Object System.Collections.Generic.List[int]
  $ids.Add($root)
  $changed = $true
  while ($changed) {
    $changed = $false
    foreach ($p in $all) {
      if ($ids.Contains([int]$p.ParentProcessId) -and -not $ids.Contains([int]$p.ProcessId)) { $ids.Add([int]$p.ProcessId); $changed = $true }
    }
  }
  $ids | ForEach-Object { $id = $_; $all | Where-Object { $_.ProcessId -eq $id } }
}

function Kind($p) {
  if ($p.Name -eq 'openmp3.exe') { return 'engine' }
  if ($p.CommandLine -match '--utility-sub-type=([a-zA-Z]+)\.') { return 'utility:' + $matches[1] }
  if ($p.CommandLine -match '--type=([a-z-]+)') { return $matches[1] }
  'webview-browser'
}

function Smtc-Play {
  $asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
  function Await($op, [Type]$t) { $task = $asTask.MakeGenericMethod($t).Invoke($null, @($op)); $task.Wait(5000) | Out-Null; $task.Result }
  [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime] | Out-Null
  $mgr = Await ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager]::RequestAsync()) ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager])
  $s = $mgr.GetSessions() | Where-Object { $_.SourceAppUserModelId -like 'openmp3*' } | Select-Object -First 1
  if (-not $s) { return "no SMTC session" }
  Await ($s.TryPlayAsync()) ([bool]) | Out-Null
  Start-Sleep -Seconds 3
  "status=" + $s.GetPlaybackInfo().PlaybackStatus
}

$results = @()
for ($i = 1; $i -le $Runs; $i++) {
  Stop-App
  Remove-Item $startupFile -ErrorAction SilentlyContinue
  $proc = Start-Process -FilePath $Exe -PassThru
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while (-not (Test-Path $startupFile) -and $sw.ElapsedMilliseconds -lt 15000) { Start-Sleep -Milliseconds 25 }
  $startup = if (Test-Path $startupFile) { (Get-Content $startupFile -Raw).Trim().Split(' ')[0] } else { "timeout" }
  Start-Sleep -Seconds 6
  $note = ""
  if ($Mode -eq "playing") { $note = Smtc-Play }
  if ($Mode -eq "tray") {
    $h = (Get-Process -Id $proc.Id).MainWindowHandle
    [MeasureW]::PostMessage($h, 0x10, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null  # WM_CLOSE → close to tray
    $note = "closed to tray"
  }
  Start-Sleep -Seconds $Settle
  $tree = Get-TreeInfo $proc.Id
  $procs = $tree | ForEach-Object { Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue } | Where-Object { $_ }
  $cpu0 = ($procs | ForEach-Object { $_.TotalProcessorTime.TotalMilliseconds } | Measure-Object -Sum).Sum
  $t0 = Get-Date
  Start-Sleep -Seconds 15
  $procs2 = $tree | ForEach-Object { Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue } | Where-Object { $_ }
  $cpu1 = ($procs2 | ForEach-Object { $_.TotalProcessorTime.TotalMilliseconds } | Measure-Object -Sum).Sum
  $elapsed = ((Get-Date) - $t0).TotalMilliseconds
  $cores = [Environment]::ProcessorCount
  $total = 0; $detail = @()
  foreach ($p in (Get-TreeInfo $proc.Id)) {
    $wp = (Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "IDProcess=$($p.ProcessId)" -ErrorAction SilentlyContinue).WorkingSetPrivate
    if ($wp) { $total += $wp; $detail += ('{0}={1:N1}' -f (Kind $p), ($wp / 1MB)) }
  }
  $r = [pscustomobject]@{
    run = $i; mode = $Mode; startup_ms = $startup; procs = $detail.Count
    taskmgr_mb = [math]::Round($total / 1MB, 1)
    cpu_machine_pct = [math]::Round(($cpu1 - $cpu0) / ($elapsed * $cores) * 100, 2)
    cpu_one_core_pct = [math]::Round(($cpu1 - $cpu0) / $elapsed * 100, 1)
    note = $note
  }
  $results += $r
  Write-Host ($r | Format-List | Out-String).Trim()
  Write-Host ("  per process: " + ($detail -join "  "))
}
Stop-App
$results | Format-Table run, mode, startup_ms, procs, taskmgr_mb, cpu_machine_pct, cpu_one_core_pct, note -AutoSize | Out-String -Width 220
