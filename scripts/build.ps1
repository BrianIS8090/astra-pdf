param([switch]$Test, [switch]$Release)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$env:CARGO_HOME = Join-Path $projectRoot 'tools\cargo'
$env:RUSTUP_HOME = Join-Path $projectRoot 'tools\rustup'
$env:PATH = "$projectRoot\tools\cargo\bin;$projectRoot\tools\w64devkit\bin;$env:PATH"
$env:ASTRA_PDFIUM_PATH = Join-Path $projectRoot 'vendor\pdfium\bin\pdfium.dll'
Push-Location $projectRoot
try {
  if ($Test) {
    cargo test --locked -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'Тесты завершились с ошибкой' }
  }
  if ($Release) {
    cargo build --locked --release
  } elseif (!$Test) {
    cargo build --locked
  }
  if ($LASTEXITCODE -ne 0) { throw 'Сборка или тесты завершились с ошибкой' }
} finally {
  Pop-Location
}
