# Runs only synthetic fixture workers. Never starts the capture worker binary.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$ValidationRoot,
    [ValidateSet('x64','x86')][string]$Arch='x64',
    [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$')][string]$Toolchain='1.97.1'
)
$ErrorActionPreference='Stop'
if ($ValidationRoot -match '[%!^"\r\n]' -or $ValidationRoot.StartsWith('\\')) { throw 'Windows-local validation path required' }
$project=Join-Path $ValidationRoot 'project'
$target=if ($Arch -eq 'x64') {'x86_64-pc-windows-msvc'} else {'i686-pc-windows-msvc'}
$vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vs=& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if ($LASTEXITCODE -ne 0 -or -not $vs) {throw 'MSVC required'}
$vcvars=Join-Path $vs 'VC\Auxiliary\Build\vcvarsall.bat'
$cargo=Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
foreach ($path in @($project,$vcvars,$cargo)) {if ($path -match '[%!^"\r\n]') {throw 'Unsupported cmd path'}}
$lines=@('@echo off','setlocal DisableDelayedExpansion',
 ('call "'+$vcvars+'" '+$Arch),'if errorlevel 1 exit /b %errorlevel%',
 ('cd /d "'+$project+'"'),('set "CARGO_HOME='+ (Join-Path $ValidationRoot 'cargo-home')+'"'),
 ('set "CARGO_TARGET_DIR='+ (Join-Path $ValidationRoot 'target')+'"'),
 ('"'+$cargo+'" +'+$Toolchain+' fmt --all --check > "'+(Join-Path $ValidationRoot 'fmt.log')+'" 2>&1'),'if errorlevel 1 exit /b %errorlevel%',
 ('"'+$cargo+'" +'+$Toolchain+' build --locked --offline -p keyweave-host --bin keyweave-host --example check_host_worker --target '+$target+' > "'+(Join-Path $ValidationRoot 'host-build.log')+'" 2>&1'),
 'if errorlevel 1 exit /b %errorlevel%','exit /b 0')
$cmd=Join-Path $ValidationRoot 'host-build.cmd'
[IO.File]::WriteAllText($cmd,($lines -join "`r`n"),(New-Object Text.UTF8Encoding $false))
Push-Location -LiteralPath $ValidationRoot
try {
    & $env:ComSpec /d /c $cmd
    $buildExit=$LASTEXITCODE
} finally { Pop-Location }
if ($buildExit -ne 0) {throw 'host fixture build failed'}
$hostExe=Join-Path $ValidationRoot ('target\'+$target+'\debug\keyweave-host.exe')
$fixture=Join-Path $ValidationRoot ('target\'+$target+'\debug\examples\check_host_worker.exe')
$casesRoot=Join-Path $ValidationRoot ('host-cases-'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $casesRoot | Out-Null
$config=Join-Path $casesRoot 'synthetic.jsonc'
[IO.File]::WriteAllText($config,'{"profiles":[{"name":"host fixture","selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]}]}]}',(New-Object Text.UTF8Encoding $false))
$results=@()
foreach ($mode in @('active','standby','stop_stall','startup_stall','invalid_pe')) {
    $dir=Join-Path $casesRoot $mode
    New-Item -ItemType Directory -Path $dir | Out-Null
    $worker=$fixture
    if ($mode -eq 'invalid_pe') { $worker=Join-Path $dir 'bad.exe';[IO.File]::WriteAllBytes($worker,[byte[]]@(0,1,2,3)) }
    $info=New-Object Diagnostics.ProcessStartInfo
    $info.FileName=$hostExe
    $info.Arguments='run-windows --config "'+$config+'" --identity strict --worker "'+$worker+'" --artifact-directory "'+$dir+'"'
    $info.UseShellExecute=$false;$info.CreateNoWindow=$true
    $info.RedirectStandardInput=$true;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
    $info.EnvironmentVariables['KEYWEAVE_HOST_FIXTURE_MODE']=$mode
    $p=New-Object Diagnostics.Process
    $p.StartInfo=$info
    try {
        if (-not $p.Start()) {throw 'owned host start failed'}
        $out=$p.StandardOutput.ReadToEndAsync();$err=$p.StandardError.ReadToEndAsync()
        if ($mode -ne 'invalid_pe') {
            Start-Sleep -Milliseconds 750;$p.StandardInput.WriteLine('');$p.StandardInput.Flush()
            $p.StandardInput.Close()
        }
        if (-not $p.WaitForExit(15000)) {
            $p.Kill();$null=$p.WaitForExit(5000);throw 'owned host did not exit within fixture limit'
        }
        $stdout=$out.GetAwaiter().GetResult();$stderr=$err.GetAwaiter().GetResult()
        [IO.File]::WriteAllText((Join-Path $dir 'stdout.log'),$stdout)
        [IO.File]::WriteAllText((Join-Path $dir 'stderr.log'),$stderr)
        $expected=if ($mode -in @('active','standby')) {0} else {1}
        if ($p.ExitCode -ne $expected) {throw ('unexpected host exit: '+$mode+' '+$p.ExitCode+' '+$stdout+' '+$stderr)}
        if ($mode -in @('active','standby')) {
            if ($stdout -notmatch 'worker_exit=Some\(0\), reason=Some\(Cooperative\), resources_complete=true') {throw 'natural stop/cleanup not confirmed'}
            if ($mode -eq 'active' -and ($stdout -notmatch 'progress_epoch=([0-9]+)' -or [UInt64]$Matches[1] -eq 0)) {throw 'active fixture progress not observed'}
            if ($mode -eq 'standby' -and $stdout -notmatch 'progress_epoch=0,') {throw 'standby minted progress'}
        } elseif ($mode -eq 'invalid_pe') {
            if ($stderr -notmatch 'worker startup failed:.*resources_complete=true') {throw 'invalid PE explicit rollback not confirmed'}
        } else {
            if ($stdout -notmatch 'worker_exit=Some\(3763033857\).*resources_complete=true') {throw 'forced owned exit/cleanup not confirmed'}
        }
        $deadline=[DateTime]::UtcNow.AddSeconds(2)
        while (@(Get-ChildItem -LiteralPath $dir -Filter 'keyweave-config-*.jsonc').Count -ne 0) {
            if ([DateTime]::UtcNow -ge $deadline) {throw 'owned sealed image deletion not observed'}
            Start-Sleep -Milliseconds 10
        }
        $results+=[pscustomobject]@{mode=$mode;host_exit=$p.ExitCode;stdout=$stdout.Trim();stderr=$stderr.Trim();owned_image_notfound=$true}
        Write-Output ('host fixture passed: '+$Arch+' '+$mode)
    } finally { $p.Dispose() }
}
$results | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $ValidationRoot 'host-results.json') -Encoding UTF8
Write-Output ('HostCasesRoot='+$casesRoot)
