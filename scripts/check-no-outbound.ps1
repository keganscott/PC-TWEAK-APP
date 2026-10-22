# Phase 5 gate (plan section 6): the app must open no outbound connections.
#
# Windows records every connection the firewall platform allows or blocks
# (Security log events 5156 / 5157) once "Filtering Platform Connection"
# auditing is on. This script turns that on, runs the app for a while (its
# startup scan included), follows the process tree it starts (WebView2 runs
# in child processes), and lists every connection made by any of them.
#
# A positive control comes first: this script makes one outbound connection
# itself and requires Windows to have logged it, so "nothing logged" cannot
# just mean auditing was off.
#
# Fails on any non-loopback connection by peaktweaks.exe itself. Connections by
# its WebView2 child processes are reported, and fail only with
# -FailOnWebView: WebView2 is Microsoft's component (NOTES.md N40), and what it
# does is a fact to measure before deciding what the Store listing may say.
#
# Needs an elevated shell (auditpol, Security log). Usage:
#   ./scripts/check-no-outbound.ps1 -Exe target\release\peaktweaks.exe [-Seconds 60] [-FailOnWebView]
param(
  [Parameter(Mandatory = $true)][string]$Exe,
  [int]$Seconds = 60,
  [switch]$FailOnWebView
)
$ErrorActionPreference = 'Stop'

$auditGuid = '{0CCE9226-69AE-11D9-BED3-505054503030}' # Filtering Platform Connection
auditpol /set /subcategory:$auditGuid /success:enable /failure:enable | Out-Null
if ($LASTEXITCODE -ne 0) { throw "auditpol could not enable connection auditing" }

function Get-Connections([datetime]$Since, [hashtable]$Pids) {
  $events = Get-WinEvent -FilterHashtable @{ LogName = 'Security'; Id = 5156, 5157; StartTime = $Since } -ErrorAction SilentlyContinue
  foreach ($e in $events) {
    $d = @{}
    foreach ($n in ([xml]$e.ToXml()).Event.EventData.Data) { $d[$n.Name] = $n.'#text' }
    $id = [int]$d['ProcessID']
    if (-not $Pids.ContainsKey($id)) { continue }
    $dest = $d['DestAddress']
    [pscustomobject]@{
      Process   = $Pids[$id]
      Pid       = $id
      Allowed   = $e.Id -eq 5156
      Direction = if ($d['Direction'] -eq '%%14593') { 'out' } else { 'in' }
      Remote    = "${dest}:$($d['DestPort'])"
      Protocol  = $d['Protocol']
      Loopback  = $dest -match '^(127\.|::1$|0:0:0:0:0:0:0:1$)'
    }
  }
}

# Positive control.
$controlStart = (Get-Date).AddSeconds(-1)
$client = [Net.Sockets.TcpClient]::new()
try { [void]$client.ConnectAsync('github.com', 443).Wait(10000) } finally { $client.Dispose() }
Start-Sleep -Seconds 2
$control = @(Get-Connections $controlStart @{ $PID = 'this script' } | Where-Object { $_.Direction -eq 'out' -and -not $_.Loopback })
if ($control.Count -eq 0) { throw "positive control failed: Windows logged no connection for this script's own outbound connect, so the audit cannot be trusted" }
Write-Host "positive control: Windows logged this script's connection to $($control[0].Remote)"

# The app and everything it starts. Windows reuses the number of a process
# that has ended, so each one is kept with when it started: a child counts
# only if it started after its parent, and only a process whose number and
# start time both still match is stopped at the end. (Stopping by number
# alone once ended this step itself, run 37998699235.)
$start = (Get-Date).AddSeconds(-1)
$app = Start-Process -FilePath $Exe -PassThru
$pids = @{ $app.Id = 'peaktweaks.exe' }
$born = @{}
$deadline = (Get-Date).AddSeconds($Seconds)
while ((Get-Date) -lt $deadline) {
  $procs = @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name, CreationDate)
  foreach ($p in $procs) {
    if ([int]$p.ProcessId -eq $app.Id -and -not $born.ContainsKey($app.Id)) { $born[$app.Id] = $p.CreationDate }
  }
  do {
    $added = $false
    foreach ($p in $procs) {
      $id = [int]$p.ProcessId
      $parent = [int]$p.ParentProcessId
      if ($id -ne $PID -and $born.ContainsKey($parent) -and -not $pids.ContainsKey($id) -and $p.CreationDate -ge $born[$parent]) {
        $pids[$id] = $p.Name
        $born[$id] = $p.CreationDate
        $added = $true
      }
    }
  } while ($added)
  if ($app.HasExited) { throw "peaktweaks.exe exited early with code $($app.ExitCode)" }
  Start-Sleep -Milliseconds 500
}
# The app through its own process object, which cannot point at another one.
Stop-Process -InputObject $app -Force -ErrorAction SilentlyContinue
foreach ($p in @(Get-CimInstance Win32_Process -Property ProcessId, CreationDate)) {
  $id = [int]$p.ProcessId
  if ($id -ne $PID -and $id -ne $app.Id -and $born.ContainsKey($id) -and $p.CreationDate -eq $born[$id]) {
    Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
  }
}
Start-Sleep -Seconds 2

$all = @(Get-Connections $start $pids)
$names = ($pids.GetEnumerator() | Group-Object Value | ForEach-Object { "$($_.Name) x$($_.Count)" }) -join ', '
Write-Host "watched for $Seconds s: $($pids.Count) processes ($names)"
Write-Host "connections logged for them: $($all.Count) (loopback $(@($all | Where-Object Loopback).Count))"
$all | Sort-Object Process, Remote | Format-Table Process, Pid, Direction, Allowed, Protocol, Remote, Loopback -AutoSize | Out-String -Width 200 | Write-Host

$ours = @($all | Where-Object { $_.Process -eq 'peaktweaks.exe' -and -not $_.Loopback })
$webview = @($all | Where-Object { $_.Process -ne 'peaktweaks.exe' -and -not $_.Loopback })
if ($ours.Count -gt 0) { throw "peaktweaks.exe made $($ours.Count) non-loopback connection(s); see the table above" }
Write-Host "OK: peaktweaks.exe made no network connection"
if ($webview.Count -gt 0) {
  $msg = "WebView2 child processes made $($webview.Count) non-loopback connection(s): $((($webview | ForEach-Object Remote) | Sort-Object -Unique) -join ', ')"
  if ($FailOnWebView) { throw $msg }
  Write-Host "REPORT: $msg"
} else {
  Write-Host "OK: its WebView2 child processes made no network connection either"
}
