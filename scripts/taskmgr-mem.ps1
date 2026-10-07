# Task Manager-style "Memory (private working set)" for the running mp3palace tree.
$root = (Get-Process mp3palace | Select-Object -First 1).Id
$all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, CommandLine
$ids = @($root); $changed = $true
while ($changed) { $changed = $false; foreach ($p in $all) { if ($ids -contains [int]$p.ParentProcessId -and -not ($ids -contains [int]$p.ProcessId)) { $ids += [int]$p.ProcessId; $changed = $true } } }
$total = 0
foreach ($id in $ids) {
  $wp = (Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "IDProcess=$id" -ErrorAction SilentlyContinue).WorkingSetPrivate
  $info = $all | Where-Object { $_.ProcessId -eq $id }
  $kind = if ($info.Name -eq 'mp3palace.exe') { 'engine' } elseif ($info.CommandLine -match '--type=([a-z-]+)') { $matches[1] } else { 'webview-browser' }
  if ($info.CommandLine -match '--utility-sub-type=([a-zA-Z]+)\.') { $kind += ":" + $matches[1] }
  '{0,-26} {1,7:N1} MB' -f $kind, ($wp / 1MB)
  $total += $wp
}
'{0,-26} {1,7:N1} MB' -f 'TOTAL (private WS)', ($total / 1MB)
