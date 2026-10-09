# Windows-only build validation; never install Hook/driver or call SendInput.
# WSL callers must use Windows-local cache, target and source snapshot.
[CmdletBinding()]
param(
    [ValidateSet('x64', 'x86')][string]$Arch = 'x64',
    [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$')][string]$Toolchain = '1.97.1',
    [string]$SourceRoot,
    [string]$SeedCargoHome,
    [switch]$Offline,
    [switch]$SdkOnly
)
$ErrorActionPreference = 'Stop'
if (-not $SourceRoot) { $SourceRoot = Split-Path -Parent $PSScriptRoot }
function CmdPath([string]$Value) {
    if ($Value -match '[%!^"\r\n]') { throw 'Unsupported cmd.exe path characters' }
    return '"' + $Value + '"'
}
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vsPath = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if ($LASTEXITCODE -ne 0 -or -not $vsPath) { throw 'MSVC x86/x64 Build Tools required' }
$vcvars = Join-Path $vsPath 'VC\Auxiliary\Build\vcvarsall.bat'
if (-not (Test-Path -LiteralPath $vcvars -PathType Leaf)) { throw 'vcvarsall.bat missing' }
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
$cargo = Join-Path $cargoBin 'cargo.exe'
if (-not $SdkOnly) {
    $installed = & (Join-Path $cargoBin 'rustup.exe') toolchain list
    if ($LASTEXITCODE -ne 0 -or -not ($installed -match ('^' + [regex]::Escape($Toolchain) + '-'))) {
        throw "Toolchain $Toolchain is not installed; choose an installed toolchain explicitly"
    }
}
$validationRoot = Join-Path $env:TEMP ('keyweave-validation-' + [Guid]::NewGuid().ToString('N'))
if ($validationRoot.StartsWith('\\')) { throw 'Windows-local TEMP required' }
New-Item -ItemType Directory -Path $validationRoot | Out-Null
$project = Join-Path $validationRoot 'project'
New-Item -ItemType Directory -Path $project | Out-Null
# Explicit source selection excludes .git, user credentials and workspace target.
foreach ($item in @('Cargo.toml','Cargo.lock','rust-toolchain.toml','crates','tests','fixtures','examples','tools')) {
    $source = Join-Path $SourceRoot $item
    if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination $project -Recurse }
}
$cargoCache = Join-Path $validationRoot 'cargo-home'
New-Item -ItemType Directory -Path $cargoCache | Out-Null
if ($SeedCargoHome) {
    foreach ($item in @('registry\cache','registry\index')) {
        $source = Join-Path $SeedCargoHome $item
        if (Test-Path -LiteralPath $source) {
            $destination = Join-Path $cargoCache (Split-Path -Parent $item)
            New-Item -ItemType Directory -Path $destination -Force | Out-Null
            Copy-Item -LiteralPath $source -Destination $destination -Recurse
        }
    }
}
$manifest = @(Get-ChildItem -LiteralPath $project -File -Recurse | Sort-Object FullName | ForEach-Object {
    [pscustomobject]@{ path = $_.FullName.Substring($project.Length + 1); sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash }
})
$manifest | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $validationRoot 'source-manifest.json') -Encoding UTF8
$target = if ($Arch -eq 'x64') { 'x86_64-pc-windows-msvc' } else { 'i686-pc-windows-msvc' }
$pointer = if ($Arch -eq 'x64') { 8 } else { 4 }
$offlineArg = if ($Offline) { ' --offline' } else { '' }
$lines = @('@echo off','setlocal DisableDelayedExpansion','chcp 65001 >nul',
    ('call ' + (CmdPath $vcvars) + ' ' + $Arch),'if errorlevel 1 exit /b %errorlevel%',
    'echo VCToolsVersion=%VCToolsVersion%','echo WindowsSDKVersion=%WindowsSDKVersion%',
    ('cd /d ' + (CmdPath $project)),
    ('cl.exe /nologo /W4 /WX /std:c11 /Zs /DKEYWEAVE_EXPECT_PTR=' + $pointer + ' tools\windows_input_abi.c'),
    'if errorlevel 1 exit /b %errorlevel%')
if (-not $SdkOnly) {
    $lines += @(
        ('set "CARGO_HOME=' + $cargoCache + '"'),
        ('set "CARGO_TARGET_DIR=' + (Join-Path $validationRoot 'target') + '"'),
        ((CmdPath $cargo) + ' +' + $Toolchain + ' -V'),
        ((CmdPath (Join-Path $cargoBin 'rustc.exe')) + ' +' + $Toolchain + ' -vV'),
        ((CmdPath $cargo) + ' +' + $Toolchain + ' test --locked' + $offlineArg + ' --workspace --target ' + $target + ' > ' + (CmdPath (Join-Path $validationRoot 'tests.log')) + ' 2>&1'),
        'if errorlevel 1 exit /b %errorlevel%',
        ((CmdPath $cargo) + ' +' + $Toolchain + ' clippy --locked' + $offlineArg + ' --workspace --all-targets --target ' + $target + ' -- -D warnings > ' + (CmdPath (Join-Path $validationRoot 'clippy.log')) + ' 2>&1'),
        'if errorlevel 1 exit /b %errorlevel%')
}
$lines += 'exit /b 0'
# Windows PowerShell Encoding.UTF8 has a BOM; cmd.exe needs BOM-free UTF-8.
[IO.File]::WriteAllText((Join-Path $validationRoot 'validate.cmd'), ($lines -join "`r`n"), (New-Object Text.UTF8Encoding $false))
Write-Output ('ValidationRoot=' + $validationRoot)
Push-Location -LiteralPath $validationRoot
try {
    $ErrorActionPreference = 'Continue'
    & $env:ComSpec /d /c validate.cmd
    $result = $LASTEXITCODE
} finally { Pop-Location }
foreach ($log in @('tests.log','clippy.log')) {
    $path = Join-Path $validationRoot $log
    if (Test-Path -LiteralPath $path) { Get-Content -LiteralPath $path -Tail 20 }
}
# Retain this unique directory and manifest for review; never delete other runs.
exit $result
