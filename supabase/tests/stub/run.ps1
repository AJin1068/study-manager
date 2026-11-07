# Runs the migrations and RLS SQL tests on a local PostgreSQL 16 using the Supabase stub.
#   powershell -File supabase/tests/stub/run.ps1 -PgBin <dir with psql.exe> -Port 54329
# Prints TAP lines; exit code 0 only when every test file passes AND the negative control fails
# as it must (a harness that cannot fail proves nothing).
param(
  [Parameter(Mandatory = $true)][string]$PgBin,
  [int]$Port = 54329
)
$ErrorActionPreference = "Continue"
$env:PGCLIENTENCODING = "UTF8"
$repo = (Resolve-Path "$PSScriptRoot\..\..\..").Path
$psql = Join-Path $PgBin "psql.exe"

function Invoke-Sql([string]$db, [string[]]$files) {
  $out = @()
  foreach ($f in $files) {
    $out += & $psql -h 127.0.0.1 -p $Port -U postgres -d $db -X -q -t -A -v ON_ERROR_STOP=1 -f $f 2>&1 | ForEach-Object { "$_" }
  }
  return $out
}

function New-Db([string]$db) {
  & $psql -h 127.0.0.1 -p $Port -U postgres -d postgres -X -q -c "drop database if exists $db" -c "create database $db" 2>&1 | Out-Null
  $setup = @("$PSScriptRoot\00_supabase_stub.sql", "$PSScriptRoot\01_pgtap_lite.sql")
  $migs = Get-ChildItem "$repo\supabase\migrations\*.sql" | Sort-Object Name | ForEach-Object { $_.FullName }
  $o = Invoke-Sql $db ($setup + $migs)
  $bad = $o | Where-Object { $_ -match "ERROR|FATAL" }
  return ,$bad
}

$failed = $false

$bad = New-Db "sm_verify"
if ($bad.Count -gt 0) { "MIGRATIONS FAILED"; $bad; exit 1 }
"MIGRATIONS OK ($((Get-ChildItem "$repo\supabase\migrations\*.sql").Count) files)"

$tests = Get-ChildItem "$repo\supabase\tests\rls_*.sql" | Sort-Object Name
foreach ($t in $tests) {
  $o = Invoke-Sql "sm_verify" @($t.FullName)
  $ok = @($o | Where-Object { $_ -cmatch "^ok " }).Count
  # A psql error line (any locale) is `psql:<file>:<line>: ...`; a short run shows up as ok != plan.
  $notok = @($o | Where-Object { $_ -cmatch "^not ok |^psql:|planned .* but ran|failed \d+ test" })
  $plan = ($o | Where-Object { $_ -cmatch "^1\.\.(\d+)" } | Select-Object -First 1)
  $want = if ($plan) { [int]($plan.Substring(3)) } else { -1 }
  "{0}: {1} ok of {2} planned, {3} problems" -f $t.Name, $ok, $want, $notok.Count
  if ($notok.Count -gt 0 -or $ok -ne $want) { $failed = $true; $o | Where-Object { $_ -cmatch "^not ok |^#|^psql:" } }
}

# Negative control: open the door the tests say is closed; rls_s2 must then fail.
$bad = New-Db "sm_negative"
if ($bad.Count -gt 0) { "NEGATIVE CONTROL SETUP FAILED"; $bad; exit 1 }
& $psql -h 127.0.0.1 -p $Port -U postgres -d sm_negative -X -q -c "grant select on public.devices to anon" 2>&1 | Out-Null
$o = Invoke-Sql "sm_negative" @("$repo\supabase\tests\rls_s2.sql")
$caught = @($o | Where-Object { $_ -cmatch "^not ok " }).Count
"negative control (anon granted select on devices): $caught failing assertion(s) in rls_s2"
if ($caught -lt 1) { "NEGATIVE CONTROL DID NOT FAIL - harness is not trustworthy"; $failed = $true }

if ($failed) { "RESULT: FAIL"; exit 1 }
"RESULT: PASS (stub PostgreSQL; partial evidence)"
exit 0
