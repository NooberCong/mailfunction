# Runs the tests that need real servers (sync, sending, and the live window) against Dovecot and
# mailpit in Docker. Mailpit takes mail for example.com only, so a test can see a refusal too.
$ErrorActionPreference = 'Stop'
# It would answer the live test's notification clicks, and open that test's messages as its own.
if (Get-Process mailfunction -ErrorAction SilentlyContinue) { throw 'Close mailfunction first.' }
docker rm -f mf-dovecot mf-mailpit 2>$null | Out-Null
docker run -d --name mf-dovecot -e USER_PASSWORD=pass -p 127.0.0.1:14143:31143 `
    -v "$PSScriptRoot/dovecot-test.conf:/etc/dovecot/conf.d/99-test.conf:ro" dovecot/dovecot:2.4.5 | Out-Null
docker run -d --name mf-mailpit -p 127.0.0.1:11025:1025 -p 127.0.0.1:18025:8025 axllent/mailpit:v1.21 `
    --smtp-auth-accept-any --smtp-auth-allow-insecure --smtp-allowed-recipients '@example\.com$' | Out-Null
try {
    $deadline = (Get-Date).AddSeconds(20)
    foreach ($port in 14143, 11025) {
        while (-not (Test-NetConnection 127.0.0.1 -Port $port -InformationLevel Quiet -WarningAction SilentlyContinue)) {
            if ((Get-Date) -gt $deadline) { throw "nothing is listening on port $port" }
        }
    }
    rustup run stable-x86_64-pc-windows-msvc cargo test -p mailfunction-core --test dovecot -- --ignored --test-threads=4
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    rustup run stable-x86_64-pc-windows-msvc cargo test -p mailfunction --test editor
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    rustup run stable-x86_64-pc-windows-msvc cargo test -p mailfunction --test live
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    rustup run stable-x86_64-pc-windows-msvc cargo test -p mailfunction --test images
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    rustup run stable-x86_64-pc-windows-msvc cargo test -p mailfunction --test composer
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    rustup run stable-x86_64-pc-windows-msvc cargo test -p mailfunction --test window
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    rustup run stable-x86_64-pc-windows-msvc cargo test -p mailfunction --test tray
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
    docker rm -f mf-dovecot mf-mailpit | Out-Null
}
