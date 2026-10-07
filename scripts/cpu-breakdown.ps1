# Per-process (and per-thread for the engine) CPU of the running openmp3 tree over N seconds.
param([int]$Seconds = 15)
$root = (Get-Process openmp3 | Select-Object -First 1).Id
$all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, CommandLine
$ids = @($root); $changed = $true
while ($changed) { $changed = $false; foreach ($p in $all) { if ($ids -contains [int]$p.ParentProcessId -and -not ($ids -contains [int]$p.ProcessId)) { $ids += [int]$p.ProcessId; $changed = $true } } }
function Kind($p) { if ($p.Name -eq 'openmp3.exe') { 'engine' } elseif ($p.CommandLine -match '--utility-sub-type=([a-zA-Z]+)\.') { 'utility:' + $matches[1] } elseif ($p.CommandLine -match '--type=([a-z-]+)') { $matches[1] } else { 'webview-browser' } }
$t0 = @{}; foreach ($id in $ids) { $t0[$id] = (Get-Process -Id $id).TotalProcessorTime.TotalMilliseconds }
$th0 = @{}; foreach ($t in (Get-Process -Id $root).Threads) { $th0[$t.Id] = $t.TotalProcessorTime.TotalMilliseconds }
Start-Sleep -Seconds $Seconds
foreach ($id in $ids) {
  $p = $all | Where-Object { $_.ProcessId -eq $id }
  $d = (Get-Process -Id $id).TotalProcessorTime.TotalMilliseconds - $t0[$id]
  '{0,-22} {1,6:N2} % of one core' -f (Kind $p), ($d / ($Seconds * 10))
}
'--- engine threads (top):'
(Get-Process -Id $root).Threads | ForEach-Object { [pscustomobject]@{ id = $_.Id; pct = ($_.TotalProcessorTime.TotalMilliseconds - $th0[$_.Id]) / ($Seconds * 10) } } | Sort-Object pct -Descending | Select-Object -First 6 | ForEach-Object { '  thread {0,-7} {1,6:N2} %' -f $_.id, $_.pct }
