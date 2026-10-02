param([Parameter(Mandatory=$true)][string]$VerifiedDirectory)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$verified = [IO.Path]::GetFullPath($VerifiedDirectory)
$result = Get-Content (Join-Path $verified 'verification.json') -Raw | ConvertFrom-Json
if (!$result.automated_checks_passed) { throw 'Автоматические проверки не подтверждены' }
$origin = Join-Path $verified 'AstraPDF'
$version = (Get-Content (Join-Path $origin 'version.json') -Raw | ConvertFrom-Json).version
if ($result.native_print_range_acceptance -ne 'passed' -and $version -notmatch '-rc\.') { throw 'Нельзя упаковать стабильный выпуск без приёмки системной печати' }
if ((Get-FileHash (Join-Path $origin 'AstraPDF.exe')).Hash -ne $result.executable_sha256) { throw 'Проверенный EXE изменился' }
foreach ($entry in (Get-Content (Join-Path $verified 'source-sha256.json') -Raw | ConvertFrom-Json)) {
  if ((Get-FileHash -LiteralPath (Join-Path $projectRoot $entry.file)).Hash -ne $entry.sha256) { throw "Изменён проверенный исходник: $($entry.file)" }
}
$release = Join-Path $projectRoot ('dist\AstraPDF-' + $version)
if (Test-Path -LiteralPath $release) { throw "Каталог выпуска уже существует: $release" }
New-Item -ItemType Directory -Path $release | Out-Null
$outputDir = Join-Path $release 'AstraPDF'
Copy-Item -LiteralPath $origin -Destination $outputDir -Recurse
Copy-Item "$projectRoot\README.md", "$projectRoot\TEST_REPORT.md" -Destination $outputDir
New-Item -ItemType Directory -Path "$outputDir\verification" | Out-Null
foreach ($name in @('verification.json','source-sha256.json','engine.json','audit.json','audit.source.json','build-and-tests.txt','clippy.txt','print-verification.json')) {
  Copy-Item -LiteralPath (Join-Path $verified $name) -Destination "$outputDir\verification"
}
Copy-Item -LiteralPath (Join-Path $verified 'ui\ui-verification.json') -Destination "$outputDir\verification"
Copy-Item -LiteralPath (Join-Path $projectRoot 'RELEASE_PLAN.md') -Destination "$outputDir\verification"
$manifest = @(Get-ChildItem $outputDir -File -Recurse | Sort-Object FullName | ForEach-Object { @{ file = $_.FullName.Substring($outputDir.Length + 1); sha256 = (Get-FileHash -LiteralPath $_.FullName).Hash } })
$manifest | ConvertTo-Json -Depth 3 | Set-Content "$outputDir\SHA256.json" -Encoding UTF8
Get-ChildItem $outputDir -File -Recurse | Where-Object { $_.LastWriteTime.Year -lt 1980 } | ForEach-Object { $_.LastWriteTime = [datetime]'1980-01-01' }
$zip = Join-Path $release ('AstraPDF-' + $version + '-win-x64.zip')
Compress-Archive -Path $outputDir -DestinationPath $zip
$sourceDir = Join-Path $release 'source\astra-pdf'
New-Item -ItemType Directory -Path $sourceDir | Out-Null
foreach ($name in @('src','scripts','assets','docs','installer','.cargo','.github','Cargo.toml','Cargo.lock','build.rs','rust-toolchain.toml','rustfmt.toml','.gitignore','.gitattributes','CHANGELOG.md','README.md','TEST_REPORT.md','RELEASE_PLAN.md','LICENSE')) {
  Copy-Item -LiteralPath (Join-Path $projectRoot $name) -Destination $sourceDir -Recurse
}
# Кэш Python не является исходным кодом и в архив не включается.
$pycache = [IO.Path]::GetFullPath((Join-Path $sourceDir 'scripts\__pycache__'))
if (!$pycache.StartsWith($sourceDir + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Неверный путь кэша' }
if (Test-Path -LiteralPath $pycache) { Remove-Item -LiteralPath $pycache -Recurse -Force }
$sourceZip = Join-Path $release ('AstraPDF-' + $version + '-source.zip')
Compress-Archive -Path $sourceDir -DestinationPath $sourceZip
$extract = Join-Path $verified ('zip-check-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
Expand-Archive -LiteralPath $zip -DestinationPath $extract
$extractedApp = Join-Path $extract 'AstraPDF'
$actual = @(Get-ChildItem $extractedApp -File -Recurse | Where-Object { $_.Name -ne 'SHA256.json' })
if ($actual.Count -ne $manifest.Count) { throw 'В архиве неверное количество файлов' }
foreach ($entry in $manifest) {
  if ((Get-FileHash -LiteralPath (Join-Path $extractedApp $entry.file)).Hash -ne $entry.sha256) { throw "В архиве повреждён файл: $($entry.file)" }
}
$oldDll = $env:ASTRA_PDFIUM_PATH
$oldPath = $env:PATH
try {
  $env:ASTRA_PDFIUM_PATH = $null
  $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
  $preview = Join-Path $extract 'extracted-render.png'
  $process = Start-Process (Join-Path $extractedApp 'AstraPDF.exe') -ArgumentList '--render', ('"' + (Join-Path $extractedApp 'Демонстрация-слоёв.pdf') + '"'), ('"' + $preview + '"') -WindowStyle Hidden -PassThru
  if (!$process.WaitForExit(30000)) { Stop-Process -Id $process.Id; $process.WaitForExit(); throw 'Проверка распакованного приложения превысила 30 секунд' }
  if ($process.ExitCode -ne 0 -or (Get-Item -LiteralPath $preview).Length -lt 1000) { throw 'Распакованный пакет не отрисовал PDF' }
} finally {
  $env:ASTRA_PDFIUM_PATH = $oldDll
  $env:PATH = $oldPath
}
@{ version = $version; tested_executable_sha256 = $result.executable_sha256; archive_files_verified = $manifest.Count; clean_extraction_render = $true; binary_zip = @{ path=$zip; bytes=(Get-Item $zip).Length; sha256=(Get-FileHash $zip).Hash }; source_zip = @{ path=$sourceZip; bytes=(Get-Item $sourceZip).Length; sha256=(Get-FileHash $sourceZip).Hash }; native_print_range_acceptance=$result.native_print_range_acceptance } | ConvertTo-Json -Depth 5 | Set-Content "$release\RELEASE.json" -Encoding UTF8
Write-Output $release
