param([string]$Python = 'python', [string[]]$ExtraPdf = @())
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$runId = 'verify-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [Guid]::NewGuid().ToString('N').Substring(0, 8)
$outputDir = Join-Path $projectRoot ('test-output\' + $runId)
New-Item -ItemType Directory -Path $outputDir | Out-Null
$started = [DateTime]::UtcNow
$oldDll = $env:ASTRA_PDFIUM_PATH
$oldSmoke = $env:ASTRA_SMOKE_PRINT
$oldPath = $env:PATH
$env:CARGO_HOME = Join-Path $projectRoot 'tools\cargo'
$env:RUSTUP_HOME = Join-Path $projectRoot 'tools\rustup'
$env:PATH = "$projectRoot\tools\cargo\bin;$projectRoot\tools\w64devkit\bin;$env:PATH"
$cargo = Join-Path $projectRoot 'tools\cargo\bin\cargo.exe'
function Invoke-Viewer([string[]]$Arguments) {
  $process = Start-Process -FilePath $exe -ArgumentList $Arguments -WindowStyle Hidden -PassThru
  if (!$process.WaitForExit(45000)) {
    # Это прекращение конкретной проверки; повторный экземпляр не запускается.
    Stop-Process -Id $process.Id
    $process.WaitForExit()
    throw "Проверка превысила 45 секунд: $Arguments"
  }
  if ($process.ExitCode -ne 0) { throw "Просмотрщик завершился с кодом $($process.ExitCode): $Arguments" }
}
function Quoted([string]$Value) { return '"' + $Value + '"' }
function Source-Hashes {
  $files = @(Get-ChildItem "$projectRoot\src", "$projectRoot\scripts", "$projectRoot\.cargo", "$projectRoot\assets" -File -Recurse | Where-Object { $_.Extension -notin @('.pyc') })
  $files += @(Get-Item "$projectRoot\Cargo.toml", "$projectRoot\Cargo.lock", "$projectRoot\build.rs", "$projectRoot\rust-toolchain.toml", "$projectRoot\rustfmt.toml", "$projectRoot\CHANGELOG.md", "$projectRoot\docs\EDITING.txt")
  return @($files | Sort-Object FullName | ForEach-Object { @{ file = $_.FullName.Substring($projectRoot.Length + 1); sha256 = (Get-FileHash -LiteralPath $_.FullName).Hash } })
}
Push-Location $projectRoot
try {
  $sourceBefore = Source-Hashes | ConvertTo-Json -Depth 3 -Compress
  & $cargo fmt --all --check
  if ($LASTEXITCODE -ne 0) { throw 'Не пройдена проверка форматирования' }
  & $cargo clippy --locked --all-targets -- -D warnings 2>&1 | Tee-Object "$outputDir\clippy.txt"
  if ($LASTEXITCODE -ne 0) { throw 'Не пройдена статическая проверка Rust' }
  & "$PSScriptRoot\build.ps1" -Test -Release 2>&1 | Tee-Object "$outputDir\build-and-tests.txt"
  & "$PSScriptRoot\audit.ps1" -Output "$outputDir\audit.json"
  & $Python "$PSScriptRoot\make_corpus.py" "$outputDir\corpus" | Set-Content "$outputDir\corpus-log.json" -Encoding UTF8
  if ($LASTEXITCODE -ne 0) { throw 'Не удалось создать проверочные документы' }
  $portable = Join-Path $outputDir 'AstraPDF'
  & "$PSScriptRoot\assemble.ps1" -Destination $portable
  $exe = Join-Path $portable 'AstraPDF.exe'
  $env:ASTRA_PDFIUM_PATH = $null
  $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
  $demo = Join-Path $outputDir 'Слои и страницы.pdf'
  Invoke-Viewer @('--demo', (Quoted $demo))
  & $Python "$PSScriptRoot\verify_theme.py" $exe $demo "$outputDir\theme-ui"
  if ($LASTEXITCODE -ne 0) { throw 'Не прошла проверка тем, сохранения выбора и неизменности PDF' }
  $sourceHash = (Get-FileHash -LiteralPath $demo).Hash
  Copy-Item -LiteralPath $demo -Destination "$portable\Демонстрация-слоёв.pdf"
  Invoke-Viewer @('--engine-selftest', (Quoted $demo), (Quoted "$outputDir\engine.json"))
  & $Python "$PSScriptRoot\verify_editing.py" $exe "$outputDir\editing"
  if ($LASTEXITCODE -ne 0) { throw 'Не прошла независимая проверка редактирования и очистки PDF' }
  & $Python "$PSScriptRoot\verify_pixel_ui.py" $exe "$outputDir\editing" "$outputDir\pixel-ui"
  if ($LASTEXITCODE -ne 0) { throw 'Не прошла проверка пикселизации в окне' }
  & $Python "$PSScriptRoot\verify_editor_ui.py" $exe "$outputDir\editing" "$outputDir\editor-ui"
  if ($LASTEXITCODE -ne 0) { throw 'Не прошла проверка инструментов и отмены' }
  & $Python "$PSScriptRoot\verify_session_ui.py" $exe "$outputDir\editing\source.pdf" "$outputDir\session-ui"
  if ($LASTEXITCODE -ne 0) { throw 'Не прошла проверка сеанса редактирования и сохранения' }
  & $Python "$PSScriptRoot\verify_fonts_ui.py" $exe "$outputDir\fonts-ui"
  if ($LASTEXITCODE -ne 0) { throw 'Не прошла проверка сохранения шрифтов и оформления' }
  & $Python "$PSScriptRoot\verify_inline_ui.py" $exe "$outputDir\fonts-ui\fonts.pdf" "$outputDir\inline-ui"
  if ($LASTEXITCODE -ne 0) { throw 'Не прошла проверка текста на странице и изменения размеров' }
  & $Python "$PSScriptRoot\verify_edit_refresh.py" $exe "$outputDir\fonts-ui\fonts.pdf" "$outputDir\edit-refresh-ui"
  if ($LASTEXITCODE -ne 0) { throw 'Не прошла проверка обновления страницы без мерцания' }
  & $Python "$PSScriptRoot\verify_pages_ui.py" $exe $demo "$outputDir\pages-ui"
  if ($LASTEXITCODE -ne 0) { throw 'Не прошла проверка управления страницами' }
  & $Python "$PSScriptRoot\verify_review_ui.py" $exe $demo "$outputDir\review-ui"
  if ($LASTEXITCODE -ne 0) { throw 'Не прошла проверка замечаний и измерений' }
  & $Python "$PSScriptRoot\verify_navigation_ui.py" $exe "$outputDir\navigation-ui"
  if ($LASTEXITCODE -ne 0) { throw 'Не прошла проверка ссылок, закладок и места чтения' }
  & $Python "$PSScriptRoot\verify_zoom_return.py" $exe "$outputDir\zoom-return"
  if ($LASTEXITCODE -ne 0) { throw 'Не прошла проверка возврата масштаба с видимыми соседними страницами' }
  $env:ASTRA_SMOKE_PRINT = '1'
  Invoke-Viewer @('--ui-smoke', (Quoted $demo), (Quoted "$outputDir\window.png"))
  $env:ASTRA_SMOKE_PRINT = $null
  Invoke-Viewer @('--print-to-pdf', (Quoted $demo), (Quoted "$outputDir\printed.pdf"))
  & $Python "$PSScriptRoot\verify_print.py" $outputDir
  if ($LASTEXITCODE -ne 0) { throw 'Не прошла независимая проверка печати' }
  & $Python "$PSScriptRoot\verify_ui.py" $exe "$outputDir\corpus" "$outputDir\ui" $demo @ExtraPdf 2>&1 | Tee-Object "$outputDir\ui.txt"
  if ($LASTEXITCODE -ne 0) { throw 'Не прошла проверка интерфейса' }
  & $Python "$PSScriptRoot\verify_detail.py" $exe "$outputDir\detail" @ExtraPdf 2>&1 | Tee-Object "$outputDir\detail.txt"
  if ($LASTEXITCODE -ne 0) { throw 'Не прошла проверка детального просмотра' }
  foreach ($file in @('engine.json', 'window.png', 'window.ok.txt', 'window.print.pdf', 'printed.pdf', 'print-verification.json', 'ui\ui-verification.json', 'detail\detail-verification.json')) {
    $result = Get-Item -LiteralPath (Join-Path $outputDir $file)
    if ($result.Length -eq 0 -or $result.LastWriteTimeUtc -lt $started.AddSeconds(-2)) { throw "Нет свежего результата: $file" }
  }
  $engine = Get-Content "$outputDir\engine.json" -Raw | ConvertFrom-Json
  foreach ($check in @('open_render','crash_recovery','timeout_recovery','memory_limit_recovery','cancellation','changed_source_rejected')) {
    if ($engine.$check -ne $true) { throw "Не подтверждена проверка движка: $check" }
  }
  if ((Get-FileHash -LiteralPath $demo).Hash -ne $sourceHash) { throw 'Исходный PDF изменён' }
  $sourceAfter = Source-Hashes | ConvertTo-Json -Depth 3 -Compress
  if ($sourceBefore -ne $sourceAfter) { throw 'Исходники изменились во время проверки' }
  $sourceAfter | Set-Content "$outputDir\source-sha256.json" -Encoding UTF8
  $state = @{ automated_checks_passed = $true; native_print_range_acceptance = 'pending'; directory = $outputDir; executable_sha256 = (Get-FileHash -LiteralPath $exe).Hash; completed_at = [DateTime]::UtcNow.ToString('o'); os = (Get-CimInstance Win32_OperatingSystem | Select-Object Caption,Version,BuildNumber) }
  $state | ConvertTo-Json -Depth 4 | Set-Content "$outputDir\verification.json" -Encoding UTF8
  Write-Output $outputDir
} finally {
  $env:ASTRA_PDFIUM_PATH = $oldDll
  $env:ASTRA_SMOKE_PRINT = $oldSmoke
  $env:PATH = $oldPath
  Pop-Location
}
