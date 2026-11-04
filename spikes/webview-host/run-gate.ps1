# Runs the gate test N times per renderer. Results land in $OutDir as <backend>-<n>.out.
param([string]$OutDir = "$PSScriptRoot\..\..\target\gate", [int]$Runs = 3, [string[]]$Backends = @("winit-software", "winit-femtovg"))
$exe = "$PSScriptRoot\..\..\target\x86_64-pc-windows-msvc\release\webview-host.exe"
New-Item -ItemType Directory -Force $OutDir | Out-Null
foreach ($b in $Backends) {
    foreach ($n in 1..$Runs) {
        $base = Join-Path $OutDir "$b-$n"
        $env:SLINT_BACKEND = $b
        $env:SHOT_DIR = "$base-shots"
        $p = Start-Process -FilePath $exe -ArgumentList "--selftest" -RedirectStandardOutput "$base.out" -RedirectStandardError "$base.err" -PassThru
        if (-not $p.WaitForExit(90000)) { Stop-Process -Id $p.Id -Force; "$b-$n TIMED OUT" } else { "$b-$n exit $($p.ExitCode)" }
    }
}
