param(
  [Parameter(Mandatory=$true)][string]$PayloadDirectory,
  [string]$OutputDirectory,
  [switch]$TestIdentity
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$payload = (Resolve-Path -LiteralPath $PayloadDirectory).Path
if (!$OutputDirectory) { $OutputDirectory = Join-Path $projectRoot 'dist\installer' }
$output = [IO.Path]::GetFullPath($OutputDirectory)
$compiler = Join-Path $projectRoot 'tools\inno-7.1.0\ISCC.exe'
if (!(Test-Path -LiteralPath $compiler)) { throw 'Сначала выполните scripts\setup-installer.ps1' }
foreach ($name in @('AstraPDF.exe', 'pdfium.dll', 'version.json', 'LICENSE.txt', 'PDFium-LICENSE.txt', 'licenses')) {
  if (!(Test-Path -LiteralPath (Join-Path $payload $name))) { throw "В комплекте отсутствует $name" }
}
$version = (Get-Content -LiteralPath "$payload\version.json" -Raw | ConvertFrom-Json).version
if ($version -notmatch '^([0-9]+\.[0-9]+\.[0-9]+)(-[A-Za-z0-9.-]+)?$') { throw 'Некорректная версия комплекта' }
$numbers = $Matches[1] + '.0'
if ((Get-Item -LiteralPath "$payload\AstraPDF.exe").VersionInfo.FileVersion -ne $version) { throw 'Версия EXE не совпала с комплектом' }
if (Test-Path -LiteralPath "$payload\SHA256.json") {
  $hashes = Get-Content -LiteralPath "$payload\SHA256.json" -Raw | ConvertFrom-Json
  $entries = if ($hashes -is [array]) {
    $hashes | ForEach-Object { [PSCustomObject]@{ Name = $_.file; Value = $_.sha256 } }
  } else { $hashes.PSObject.Properties }
  foreach ($entry in $entries) {
    $file = [IO.Path]::GetFullPath((Join-Path $payload $entry.Name))
    if (!$file.StartsWith($payload.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Путь контрольной суммы вне комплекта' }
    if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $entry.Value) { throw "Не совпала контрольная сумма: $($entry.Name)" }
  }
}
New-Item -ItemType Directory -Force -Path $output | Out-Null
$arguments = @("/DPayloadDir=$payload", "/DOutputDir=$output", "/DAppVersion=$version", "/DVersionNumbers=$numbers")
if ($TestIdentity) { $arguments += '/DTestIdentity=1' }
& $compiler @arguments "$projectRoot\installer\AstraPDF.iss"
if ($LASTEXITCODE -ne 0) { throw "Не удалось собрать установщик: $LASTEXITCODE" }
$suffix = if ($TestIdentity) { '-test' } else { '' }
$installer = Join-Path $output "AstraPDF-$version-setup$suffix-x64.exe"
if (!(Test-Path -LiteralPath $installer)) { throw 'Сборка не создала установщик' }
$installerHash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
"$installerHash  $([IO.Path]::GetFileName($installer))" | Set-Content -LiteralPath "$installer.sha256" -Encoding ASCII
Write-Output "Установщик: $installer"
