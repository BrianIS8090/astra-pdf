param([Parameter(Mandatory=$true)][string]$Destination)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$outputDir = [IO.Path]::GetFullPath($Destination)
if (Test-Path -LiteralPath $outputDir) { throw "Для сборки нужна новая папка: $outputDir" }
New-Item -ItemType Directory -Path $outputDir | Out-Null
Copy-Item "$projectRoot\target\release\astra-pdf.exe" "$outputDir\AstraPDF.exe"
& "$PSScriptRoot\sign.ps1" -Path "$outputDir\AstraPDF.exe"
Copy-Item "$projectRoot\vendor\pdfium\bin\pdfium.dll" $outputDir
Copy-Item "$projectRoot\vendor\pdfium\LICENSE" "$outputDir\PDFium-LICENSE.txt"
Copy-Item "$projectRoot\vendor\pdfium\licenses" "$outputDir\licenses" -Recurse
Copy-Item "$projectRoot\assets\lucide\LICENSE.txt" "$outputDir\licenses\Lucide-LICENSE.txt"
Copy-Item "$projectRoot\LICENSE" "$outputDir\LICENSE.txt"
Copy-Item "$projectRoot\README.md" $outputDir
Copy-Item "$projectRoot\CHANGELOG.md" $outputDir
Copy-Item "$projectRoot\docs\EDITING.txt" $outputDir
$env:CARGO_HOME = Join-Path $projectRoot 'tools\cargo'
$env:RUSTUP_HOME = Join-Path $projectRoot 'tools\rustup'
$metadataText = & "$projectRoot\tools\cargo\bin\cargo.exe" metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-gnu --manifest-path "$projectRoot\Cargo.toml"
if ($LASTEXITCODE -ne 0) { throw 'Не удалось получить список зависимостей' }
$metadata = $metadataText | ConvertFrom-Json
foreach ($package in $metadata.packages) {
  if ($package.name -eq 'astra-pdf') { continue }
  $sourceDir = Split-Path -Parent $package.manifest_path
  $licenseDir = Join-Path $outputDir ("licenses\rust-" + $package.name + '-' + $package.version)
  New-Item -ItemType Directory -Force $licenseDir | Out-Null
  $files = @(Get-ChildItem -LiteralPath $sourceDir -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)' })
  if ($files.Count -eq 0) {
    $fallback = Join-Path $projectRoot ("assets\licenses\" + $package.name + '-' + $package.version)
    if (Test-Path -LiteralPath $fallback) { $files = @(Get-ChildItem -LiteralPath $fallback -File) }
  }
  if ($files.Count -eq 0) { throw "Нет файла лицензии у $($package.name)" }
  $files | Copy-Item -Destination $licenseDir
}
$metadata.packages | Select-Object name,version,license,repository | ConvertTo-Json | Set-Content "$outputDir\licenses\rust-components.json" -Encoding UTF8
Copy-Item "$projectRoot\tools\w64devkit\COPYING.MinGW-w64-runtime.txt" "$outputDir\licenses\"
Copy-Item "$projectRoot\tools\rustup\toolchains\1.99.0-x86_64-pc-windows-gnu\share\doc\COPYING.RUNTIME" "$outputDir\licenses\GCC-RUNTIME.txt"
Copy-Item "$projectRoot\tools\rustup\toolchains\1.99.0-x86_64-pc-windows-gnu\share\doc\COPYING3" "$outputDir\licenses\GCC-GPL3.txt"
$package = $metadata.packages | Where-Object { $_.name -eq 'astra-pdf' }
if ((Get-Item "$outputDir\AstraPDF.exe").VersionInfo.FileVersion -ne $package.version) { throw 'Версия ресурса EXE не совпадает с Cargo.toml' }
@{ version = $package.version; pdfium = '156.0.8076.0'; platform = 'Windows 11 x64'; built_at = [DateTime]::UtcNow.ToString('o'); source_lock_sha256 = (Get-FileHash "$projectRoot\Cargo.lock").Hash } | ConvertTo-Json | Set-Content "$outputDir\version.json" -Encoding UTF8
