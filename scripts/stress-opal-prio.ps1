# Stress-test Opal scheduler resilience:
# 1) Demote Opal + its WebView2 kids to Idle
# 2) Wait for the 2s watchdog to repair them to HIGH
# 3) Burn CPU while sampling priorities
# Requires an already-running Opal process.

$ErrorActionPreference = "Stop"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinPrio {
  [DllImport("kernel32.dll")] public static extern IntPtr OpenProcess(uint a, bool i, int pid);
  [DllImport("kernel32.dll")] public static extern bool SetPriorityClass(IntPtr h, uint c);
  [DllImport("kernel32.dll")] public static extern uint GetPriorityClass(IntPtr h);
  [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
  public const uint IDLE = 0x40;
  public const uint HIGH = 0x80;
  public const uint QUERY = 0x0400;
  public const uint SET = 0x0200;
  public const uint QUERY_LIMITED = 0x1000;
}
"@

function Get-OpalTree {
  $opal = Get-Process -Name "opal" -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $opal) { throw "opal.exe is not running - start the app first" }
  $root = $opal.Id
  $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name
  $frontier = New-Object System.Collections.Generic.Queue[int]
  $frontier.Enqueue($root)
  $seen = New-Object 'System.Collections.Generic.HashSet[int]'
  [void]$seen.Add($root)
  $pids = New-Object System.Collections.Generic.List[int]
  $pids.Add($root)
  while ($frontier.Count -gt 0) {
    $parent = $frontier.Dequeue()
    foreach ($row in $all) {
      if ($row.ParentProcessId -eq $parent -and $row.Name -eq 'msedgewebview2.exe' -and $seen.Add([int]$row.ProcessId)) {
        $pids.Add([int]$row.ProcessId)
        $frontier.Enqueue([int]$row.ProcessId)
      }
    }
  }
  return @($pids)
}

function Set-Prio([int]$procId, [uint32]$cls) {
  $h = [WinPrio]::OpenProcess([WinPrio]::SET -bor [WinPrio]::QUERY_LIMITED -bor [WinPrio]::QUERY, $false, $procId)
  if ($h -eq [IntPtr]::Zero) { return $false }
  try { return [WinPrio]::SetPriorityClass($h, $cls) } finally { [void][WinPrio]::CloseHandle($h) }
}

function Get-Prio([int]$procId) {
  $h = [WinPrio]::OpenProcess([WinPrio]::QUERY_LIMITED -bor [WinPrio]::QUERY, $false, $procId)
  if ($h -eq [IntPtr]::Zero) { return 0 }
  try { return [WinPrio]::GetPriorityClass($h) } finally { [void][WinPrio]::CloseHandle($h) }
}

function Name-Prio([uint32]$c) {
  switch ($c) {
    0x80 { "HIGH" }
    0x8000 { "ABOVE_NORMAL" }
    0x20 { "NORMAL" }
    0x4000 { "BELOW_NORMAL" }
    0x40 { "IDLE" }
    default { "0x{0:X}" -f $c }
  }
}

Write-Host "=== Opal scheduler stress ==="
$tree = @(Get-OpalTree)
Write-Host ("tree pids: " + ($tree -join ", "))
Write-Host "before:"
foreach ($p in $tree) { Write-Host ("  pid={0} prio={1}" -f $p, (Name-Prio (Get-Prio $p))) }

Write-Host "demoting entire tree to IDLE..."
foreach ($p in $tree) { [void](Set-Prio $p ([WinPrio]::IDLE)) }
Write-Host "immediately after demote:"
foreach ($p in $tree) { Write-Host ("  pid={0} prio={1}" -f $p, (Name-Prio (Get-Prio $p))) }

Write-Host "waiting 5s for watchdog repair..."
Start-Sleep -Seconds 5
$repaired = 0
$failed = 0
Write-Host "after watchdog:"
foreach ($p in $tree) {
  $c = Get-Prio $p
  $n = Name-Prio $c
  Write-Host ("  pid={0} prio={1}" -f $p, $n)
  if ($c -eq [WinPrio]::HIGH) { $repaired++ } else { $failed++ }
}

Write-Host "burning CPU 8s while sampling..."
$jobs = 1..([Math]::Max(2, [Environment]::ProcessorCount - 1)) | ForEach-Object {
  Start-Job { $end = [DateTime]::UtcNow.AddSeconds(8); while ([DateTime]::UtcNow -lt $end) { $x = 0; for ($i=0; $i -lt 200000; $i++) { $x += $i } } }
}
$samples = @()
$endAt = [DateTime]::UtcNow.AddSeconds(8)
while ([DateTime]::UtcNow -lt $endAt) {
  $root = $tree[0]
  $samples += (Get-Prio $root)
  Start-Sleep -Milliseconds 400
}
$jobs | Wait-Job | Out-Null
$jobs | Remove-Job | Out-Null

$highSamples = @($samples | Where-Object { $_ -eq [WinPrio]::HIGH }).Count
Write-Host ("CPU-burn samples HIGH={0}/{1}" -f $highSamples, $samples.Count)
Write-Host ("RESULT repaired={0} failed={1} hold_under_load={2}/{3}" -f $repaired, $failed, $highSamples, $samples.Count)

if ($failed -gt 0 -or $highSamples -lt $samples.Count) {
  exit 1
}
exit 0
