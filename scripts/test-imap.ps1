# Runs the tests that need a real server (sync, and the live window) against Dovecot in Docker.
$ErrorActionPreference = 'Stop'
docker rm -f mf-dovecot 2>$null | Out-Null
docker run -d --name mf-dovecot -e USER_PASSWORD=pass -p 127.0.0.1:14143:31143 `
    -v "$PSScriptRoot/dovecot-test.conf:/etc/dovecot/conf.d/99-test.conf:ro" dovecot/dovecot:2.4.5 | Out-Null
try {
    $deadline = (Get-Date).AddSeconds(20)
    while (-not (Test-NetConnection 127.0.0.1 -Port 14143 -InformationLevel Quiet -WarningAction SilentlyContinue)) {
        if ((Get-Date) -gt $deadline) { throw 'Dovecot did not start' }
    }
    rustup run stable-x86_64-pc-windows-msvc cargo test -p mailfunction-core --test dovecot -- --ignored --test-threads=4
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    rustup run stable-x86_64-pc-windows-msvc cargo test -p mailfunction --test live
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
    docker rm -f mf-dovecot | Out-Null
}
