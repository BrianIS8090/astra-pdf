$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$projectRoot = Split-Path -Parent $PSScriptRoot
$toolsDir = Join-Path $projectRoot 'tools'
$vendorDir = Join-Path $projectRoot 'vendor'
New-Item -ItemType Directory -Force $toolsDir, $vendorDir | Out-Null
function Get-VerifiedFile($Uri, $Path, $Hash) {
  if (!(Test-Path -LiteralPath $Path)) { Invoke-WebRequest -UseBasicParsing $Uri -OutFile $Path }
  if ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash -ne $Hash) { throw "Не совпала контрольная сумма: $Path" }
}
Get-VerifiedFile 'https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/8076/pdfium-win-x64.tgz' "$vendorDir\pdfium.tgz" '808d36da9bc5a3104315fb307c80998121f565ee53953633bf33e80d7429e5ac'
New-Item -ItemType Directory -Force "$vendorDir\pdfium" | Out-Null
tar -xzf "$vendorDir\pdfium.tgz" -C "$vendorDir\pdfium"
if ($LASTEXITCODE -ne 0) { throw 'Не удалось распаковать PDFium' }
if (!(Test-Path "$toolsDir\w64devkit\bin\gcc.exe")) {
  Get-VerifiedFile 'https://github.com/skeeto/w64devkit/releases/download/v2.10.0/w64devkit-x64-2.10.0.7z.exe' "$toolsDir\w64devkit-fast.exe" '18d0a4c71a166f8401ab6305781bec5882b40b5e06ba9807c61cb5f3b3c6325e'
  & "$toolsDir\w64devkit-fast.exe" -y "-o$toolsDir"
  if ($LASTEXITCODE -ne 0) { throw 'Не удалось распаковать инструменты Windows' }
}
if (!(Test-Path "$toolsDir\rustup-init.exe")) { Invoke-WebRequest -UseBasicParsing 'https://win.rustup.rs/x86_64' -OutFile "$toolsDir\rustup-init.exe" }
$env:CARGO_HOME = "$toolsDir\cargo"
$env:RUSTUP_HOME = "$toolsDir\rustup"
& "$toolsDir\rustup-init.exe" -y --no-modify-path --profile minimal --default-host x86_64-pc-windows-gnu --default-toolchain 1.99.0
if ($LASTEXITCODE -ne 0) { throw 'Не удалось установить Rust' }
& "$toolsDir\cargo\bin\rustup.exe" component add rustfmt clippy --toolchain 1.99.0-x86_64-pc-windows-gnu
if ($LASTEXITCODE -ne 0) { throw 'Не удалось установить средства проверки Rust' }
