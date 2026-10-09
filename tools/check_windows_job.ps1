# Runs only the no-input check_job fixture from a validated Windows-local build.
# Abruptly terminates its own parent fixture; child PID is used only to observe.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$ValidationRoot,
    [ValidateSet('x86_64-pc-windows-msvc','i686-pc-windows-msvc')]
    [string]$Target = 'x86_64-pc-windows-msvc',
    [switch]$ProgressDelivery
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $ValidationRoot).Path
if ($root.StartsWith('\\')) { throw 'Windows-local validation directory required' }
$exe = Join-Path $root ('target\' + $Target + '\debug\examples\check_job.exe')
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw 'Build check_job first' }
if ($ProgressDelivery) {
    $delivery = Join-Path $root ('target\' + $Target + '\debug\examples\check_job_progress.exe')
    if (-not (Test-Path -LiteralPath $delivery -PathType Leaf)) { throw 'Build check_job_progress first' }
    $ErrorActionPreference = 'Continue'
    & $delivery
    $deliveryCode = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($deliveryCode -ne 0) { throw ('Progress delivery smoke failed: ' + $deliveryCode) }
}
$ErrorActionPreference = 'Continue'
& $exe
$smokeCode = $LASTEXITCODE
$ErrorActionPreference = 'Stop'
if ($smokeCode -ne 0) { throw ('Explicit Job smoke failed: ' + $smokeCode) }
$marker = Join-Path $root ('job-death-' + [Guid]::NewGuid().ToString('N') + '.ready')
if ($marker -match '["\r\n]') { throw 'Invalid marker path' }
$parent = New-Object System.Diagnostics.Process
$parent.StartInfo.FileName = $exe
$parent.StartInfo.Arguments = '--fixture-parent-death "' + $marker + '"'
$parent.StartInfo.UseShellExecute = $false
$parent.StartInfo.CreateNoWindow = $true
$parent.StartInfo.RedirectStandardError = $true
$parent.StartInfo.RedirectStandardOutput = $true
$worker = $null
$started = $false
try {
    if (-not $parent.Start()) { throw 'Own parent fixture did not start' }
    $started = $true
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $workerId = 0
    while ($timer.ElapsedMilliseconds -lt 5000 -and $workerId -eq 0) {
        if ($parent.HasExited) {
            throw ('Parent fixture exited before ready: ' + $parent.StandardError.ReadToEnd())
        }
        if (Test-Path -LiteralPath $marker -PathType Leaf) {
            $file = Get-Item -LiteralPath $marker
            if ($file.Length -gt 32) { throw 'Oversized fixture marker' }
            $raw = [IO.File]::ReadAllText($marker).Trim()
            $parsedId = 0
            if ([int]::TryParse($raw, [ref]$parsedId) -and $parsedId -gt 0) {
                $workerId = $parsedId
            }
        }
        if ($workerId -eq 0) { Start-Sleep -Milliseconds 10 }
    }
    if ($workerId -eq 0) { throw 'Child fixture did not become ready' }
    $worker = [Diagnostics.Process]::GetProcessById($workerId)
    # Force and retain a process handle before stopping the parent. Later wait
    # observes that process, regardless of PID reuse. Never kill by marker PID.
    $observedHandle = $worker.Handle
    if ($observedHandle -eq [IntPtr]::Zero -or $worker.HasExited) { throw 'Child was not alive' }
    if (-not [String]::Equals($worker.MainModule.FileName, $exe, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Observed process was not the owned fixture executable'
    }
    $parent.Kill()
    if (-not $parent.WaitForExit(5000)) { throw 'Own parent termination was not observed' }
    if (-not $worker.WaitForExit(5000)) { throw 'Worker survived parent death' }
    Write-Output 'parent_death=passed child_exit=observed no_drop=true input_apis=none'
} finally {
    if ($started -and -not $parent.HasExited) {
        $parent.Kill()
        if (-not $parent.WaitForExit(5000)) { Write-Warning 'Own parent cleanup did not finish' }
    }
    if ($worker) { $worker.Dispose() }
    $parent.Dispose()
    if (Test-Path -LiteralPath $marker) { Remove-Item -LiteralPath $marker }
}
