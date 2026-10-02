param([Parameter(Mandatory=$true)][string]$Output)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$projectRoot = Split-Path -Parent $PSScriptRoot
$auditRoot = Join-Path $projectRoot 'tools\audit'
$archive = Join-Path $auditRoot 'cargo-audit.zip'
$auditExe = Join-Path $auditRoot 'cargo-audit-x86_64-pc-windows-msvc-v0.22.2\cargo-audit.exe'
New-Item -ItemType Directory -Force $auditRoot | Out-Null
if (!(Test-Path -LiteralPath $archive)) {
  Invoke-WebRequest 'https://github.com/rustsec/rustsec/releases/download/cargo-audit/v0.22.2/cargo-audit-x86_64-pc-windows-msvc-v0.22.2.zip' -OutFile $archive
}
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne '0A7316540862C13D954F648917CEACCA593747BAED6EEC180FAFA590BE2710AB') { throw 'Не совпала контрольная сумма cargo-audit' }
Expand-Archive -LiteralPath $archive -DestinationPath $auditRoot -Force
$database = Join-Path $projectRoot 'tools\advisory-db'
if (Test-Path -LiteralPath (Join-Path $database '.git')) {
  git -C $database fetch --depth 1 origin HEAD
  if ($LASTEXITCODE -ne 0) { throw 'Не удалось обновить RustSec' }
  git -C $database checkout --detach FETCH_HEAD
} else {
  git clone --depth 1 https://github.com/RustSec/advisory-db.git $database
}
if ($LASTEXITCODE -ne 0) { throw 'Не удалось подготовить RustSec' }
$result = & $auditExe audit --file (Join-Path $projectRoot 'Cargo.lock') --db $database --no-fetch --json --deny warnings
$code = $LASTEXITCODE
$result | Set-Content -LiteralPath $Output -Encoding UTF8
$commit = git -C $database rev-parse HEAD
@{ checked_at = [DateTime]::UtcNow.ToString('o'); database_commit = $commit; database_url = 'https://github.com/RustSec/advisory-db'; tool = 'cargo-audit 0.22.2'; cargo_lock_sha256 = (Get-FileHash (Join-Path $projectRoot 'Cargo.lock')).Hash } | ConvertTo-Json | Set-Content -LiteralPath ([IO.Path]::ChangeExtension($Output, 'source.json')) -Encoding UTF8
if ($code -ne 0) { throw "Проверка RustSec не пройдена: $Output" }
$parsed = $result | ConvertFrom-Json
if ($parsed.vulnerabilities.found -or $parsed.vulnerabilities.count -ne 0 -or $parsed.database.'advisory-count' -lt 1000) { throw 'Неполный или отрицательный результат аудита' }
