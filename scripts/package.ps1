# Builds the release: target\dist gets the executable, the installer and their checksums.
# Needs Inno Setup 6 (winget install JRSoftware.InnoSetup).
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$compiler = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $compiler) { throw 'Inno Setup 6 is not installed' }

$version = (Select-String -Path crates\app\Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$built = "$root\target\x86_64-pc-windows-msvc\release\mailfunction.exe"
$dist = "$root\target\dist"

rustup run stable-x86_64-pc-windows-msvc cargo build --release -p mailfunction
if ($LASTEXITCODE) { throw 'the build failed' }

Remove-Item $dist -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory $dist | Out-Null
Copy-Item $built, crates\app\ui\fonts\OFL.txt $dist

& $compiler /Q "/DVersion=$version" "/DExe=$built" "/DOutputDir=$dist" installer\mailfunction.iss
if ($LASTEXITCODE) { throw 'the installer failed to build' }

Get-ChildItem $dist\*.exe | ForEach-Object { "$((Get-FileHash $_).Hash.ToLower())  $($_.Name)" } |
    Set-Content $dist\SHA256SUMS.txt
Get-ChildItem $dist | Select-Object Name, Length
