param(
  [Parameter(Mandatory=$true)][string]$Installer,
  [Parameter(Mandatory=$true)][string]$PayloadDirectory,
  [string]$PreviousInstaller
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$installerPath = (Resolve-Path -LiteralPath $Installer).Path
$payload = (Resolve-Path -LiteralPath $PayloadDirectory).Path
if ([IO.Path]::GetFileName($installerPath) -notmatch '-setup-test-x64\.exe$') { throw 'Проверка принимает только отдельную тестовую сборку установщика' }
if ($PreviousInstaller) {
  $PreviousInstaller = (Resolve-Path -LiteralPath $PreviousInstaller).Path
  if ([IO.Path]::GetFileName($PreviousInstaller) -notmatch '-setup-test-x64\.exe$') { throw 'Для обновления нужна тестовая сборка' }
}
$output = Join-Path $projectRoot ('test-output\installer-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [Guid]::NewGuid().ToString('N').Substring(0,8))
$installDir = Join-Path $output 'Программа с пробелами'
$exe = Join-Path $installDir 'AstraPDF-InstallerTest.exe'
$product = 'AstraPDF.InstallerTest'
$name = 'Astra PDF Installer Test'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{8B24310F-20F8-494B-B9C8-3E40C37A5A5D}_is1'
$programs = [Environment]::GetFolderPath('Programs')
$desktop = [Environment]::GetFolderPath('DesktopDirectory')
$version = (Get-Content -LiteralPath "$payload\version.json" -Raw | ConvertFrom-Json).version
$shortcut = Join-Path $programs "$name\$name $version.lnk"
$desktopShortcut = Join-Path $desktop "$name $version.lnk"
$settingsShortcut = Join-Path $programs "$name\Выбрать для PDF по умолчанию.url"
$privateKeys = @("HKCU:\Software\$product", "HKCU:\Software\Classes\$product.Document", 'HKCU:\Software\Classes\Applications\AstraPDF-InstallerTest.exe', 'HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\AstraPDF-InstallerTest.exe', $uninstallKey)
$openWith = 'HKCU:\Software\Classes\.pdf\OpenWithProgids'
$registered = 'HKCU:\Software\RegisteredApplications'
$choiceKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.pdf\UserChoice'
$checks = [Collections.Generic.List[string]]::new()
if (!('AstraInstaller.Associations' -as [type])) {
  Add-Type -Path (Join-Path $PSScriptRoot 'InstallerChecks.cs')
}
function Assert-Check([bool]$Condition, [string]$Message) {
  if (!$Condition) { throw "Не пройдена проверка: $Message" }
  $checks.Add($Message)
}
function Read-Value([string]$Path, [string]$Name = '') {
  if (Test-Path -LiteralPath $Path) { return (Get-Item -LiteralPath $Path).GetValue($Name) }
  return $null
}
function Read-Choice {
  return (@{ ProgId = Read-Value $choiceKey 'ProgId'; Hash = Read-Value $choiceKey 'Hash' } | ConvertTo-Json -Compress)
}
function Run-Checked([string]$Path, [string[]]$Arguments, [int]$Timeout = 90000) {
  $process = Start-Process -FilePath $Path -ArgumentList $Arguments -WindowStyle Hidden -PassThru
  if (!$process.WaitForExit($Timeout)) { throw "Превышено время ожидания: $Path" }
  if ($process.ExitCode -ne 0) { throw "Код завершения $($process.ExitCode): $Path" }
}
function Assert-Registration {
  Assert-Check ((Read-Value $registered $name) -eq "Software\$product\Capabilities") 'Приложение зарегистрировано в Windows'
  Assert-Check ((Read-Value "HKCU:\Software\$product\Capabilities\FileAssociations" '.pdf') -eq "$product.Document") 'PDF связан с собственным ProgID'
  $command = '"' + $exe + '" "%1"'
  Assert-Check ((Read-Value "HKCU:\Software\Classes\$product.Document\shell\open\command") -eq $command) 'Команда открытия корректно заключает пути в кавычки'
  Assert-Check ((Read-Value 'HKCU:\Software\Classes\Applications\AstraPDF-InstallerTest.exe\shell\open\command') -eq $command) 'Открыть с помощью указывает на установленный EXE'
  Assert-Check (((Get-Item -LiteralPath $openWith).GetValueNames()) -contains "$product.Document") 'Программа доступна в списке открытия PDF'
  $buffer = [Text.StringBuilder]::new(2048)
  [uint32]$length = 2048
  $result = [AstraInstaller.Associations]::AssocQueryString(0, 2, "$product.Document", 'open', $buffer, [ref]$length)
  Assert-Check ($result -eq 0 -and $buffer.ToString() -eq $exe) 'Системный API Windows разрешает обработчик PDF в установленный EXE'
  $buffer.Clear() | Out-Null
  $length = 2048
  $result = [AstraInstaller.Associations]::AssocQueryString(0, 4, "$product.Document", 'open', $buffer, [ref]$length)
  $installedVersion = (Get-Item -LiteralPath $exe).VersionInfo.FileVersion
  Assert-Check ($result -eq 0 -and $buffer.ToString() -in @($name, "$name $installedVersion")) 'Windows возвращает правильное имя приложения для обработчика PDF'
  Assert-Check ((Read-Choice) -eq $choiceBefore) 'Текущий выбор PDF по умолчанию сохранён'
  Assert-Check ((Read-Value 'HKCU:\Software\Classes\Applications\AstraPDF.exe\shell\open\command') -eq $productionCommandBefore) 'Рабочая регистрация Astra PDF не затронута тестом'
}
function Assert-Payload {
  foreach ($file in Get-ChildItem -LiteralPath $payload -File -Recurse) {
    $relative = $file.FullName.Substring($payload.Length + 1)
    if ($relative -eq 'SHA256.json') { continue }
    if ($relative -eq 'AstraPDF.exe') { $relative = 'AstraPDF-InstallerTest.exe' }
    $destination = Join-Path $installDir $relative
    if (!(Test-Path -LiteralPath $destination) -or (Get-FileHash -LiteralPath $destination).Hash -ne (Get-FileHash -LiteralPath $file.FullName).Hash) {
      throw "Изменён или не установлен файл: $relative"
    }
  }
  $checks.Add('Все файлы установленного комплекта совпали побайтно')
}
foreach ($key in $privateKeys) {
  if (Test-Path -LiteralPath $key) { throw "Осталась тестовая регистрация; требуется осмотр до проверки: $key" }
}
if ((Read-Value $registered $name) -or (Test-Path -LiteralPath (Join-Path $programs $name)) -or (Test-Path -LiteralPath $desktopShortcut)) { throw 'Тестовая установка уже существует' }
if ((Test-Path -LiteralPath $openWith) -and ((Get-Item -LiteralPath $openWith).GetValueNames() -contains "$product.Document")) { throw 'Тестовый обработчик уже существует' }
$choiceBefore = Read-Choice
$productionCommandBefore = Read-Value 'HKCU:\Software\Classes\Applications\AstraPDF.exe\shell\open\command'
New-Item -ItemType Directory -Path $output | Out-Null
$completed = $false
try {
  if ($PreviousInstaller) {
    Run-Checked $PreviousInstaller @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', "/DIR=`"$installDir`"", '/TASKS="desktopicon"', "/LOG=`"$output\previous.log`"")
    Assert-Registration
    $previousVersion = (Get-Item -LiteralPath $exe).VersionInfo.FileVersion
  }
  # Воспроизводим устаревшую запись переносимой версии в «Открыть с помощью».
  $staleKey = 'HKCU:\Software\Classes\Applications\AstraPDF-InstallerTest.exe\shell\open\command'
  New-Item -Path $staleKey -Force | Out-Null
  Set-Item -LiteralPath $staleKey -Value '"C:\Old Portable\AstraPDF-InstallerTest.exe" "%1"'
  $arguments = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', "/LOG=`"$output\install.log`"")
  if (!$PreviousInstaller) { $arguments += @("/DIR=`"$installDir`"", '/TASKS="desktopicon"') }
  Run-Checked $installerPath $arguments
  Assert-Registration
  Assert-Payload
  Assert-Check (!(Test-Path -LiteralPath (Join-Path $programs "$name\$name.lnk"))) 'Старый ярлык без версии удалён'
  Assert-Check ((Read-Value "HKCU:\Software\Classes\$product.Document\Application" 'ApplicationName') -eq "$name $version") 'В выборе PDF отображается новая версия'
  Assert-Check (Test-Path -LiteralPath $shortcut) 'Создан ярлык в меню Пуск'
  Assert-Check (Test-Path -LiteralPath $desktopShortcut) 'Создан выбранный ярлык рабочего стола'
  # Оболочка может вернуть короткое имя 8.3, особенно при кириллице в другой локали.
  $shortcutTarget = [AstraInstaller.Associations]::ShortcutTarget($shortcut)
  $longTarget = [Text.StringBuilder]::new(32768)
  $targetLength = [AstraInstaller.Associations]::GetLongPathName($shortcutTarget, $longTarget, 32768)
  Assert-Check ($targetLength -gt 0 -and $targetLength -lt 32768 -and $longTarget.ToString() -eq $exe) "Ярлык запускает установленную версию: $shortcutTarget"
  Assert-Check (Test-Path -LiteralPath $settingsShortcut) 'Создан ярлык выбора PDF по умолчанию'
  Assert-Check ((Get-Content -LiteralPath $settingsShortcut -Raw) -match 'URL=ms-settings:defaultapps\?registeredAppUser=Astra%20PDF%20Installer%20Test') 'Ярлык ведёт в настройки именно этого приложения'
  $version = (Get-Content -LiteralPath "$payload\version.json" -Raw | ConvertFrom-Json).version
  Assert-Check ((Read-Value $uninstallKey 'DisplayVersion') -eq $version) 'Windows показывает версию установленного приложения'
  if ($PreviousInstaller) { Assert-Check ($previousVersion -ne $version) 'Предыдущая версия обновлена в прежней папке без повторного выбора пути' }
  # Проверяем восстановление файлов при повторной установке той же версии.
  Set-Content -LiteralPath "$installDir\pdfium.dll" -Value 'Проверка восстановления' -Encoding UTF8
  Run-Checked $installerPath @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', "/LOG=`"$output\repair.log`"")
  Assert-Payload
  Assert-Registration
  $document = Join-Path $installDir 'Мой документ с пробелами.pdf'
  Run-Checked $exe @('--demo', "`"$document`"")
  $documentHash = (Get-FileHash -LiteralPath $document).Hash
  $image = Join-Path $output 'render.png'
  $savedPath = $env:PATH
  try {
    $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
    Run-Checked $exe @('--render', "`"$document`"", "`"$image`"")
  } finally { $env:PATH = $savedPath }
  Assert-Check ((Get-Item -LiteralPath $image).Length -gt 1000) 'Установленный EXE рисует PDF без инструментов разработки в PATH'
  $uninstaller = Join-Path $installDir 'unins000.exe'
  Assert-Check (Test-Path -LiteralPath $uninstaller) 'Установлен деинсталлятор'
  Run-Checked $uninstaller @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$output\uninstall.log`"")
  $deadline = [DateTime]::UtcNow.AddSeconds(20)
  while ((Test-Path -LiteralPath $exe) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 200 }
  foreach ($key in $privateKeys) { Assert-Check (!(Test-Path -LiteralPath $key)) "Удалена собственная регистрация: $key" }
  Assert-Check ($null -eq (Read-Value $registered $name)) 'Удалено только своё имя из RegisteredApplications'
  Assert-Check (!(Test-Path -LiteralPath $openWith) -or !((Get-Item -LiteralPath $openWith).GetValueNames() -contains "$product.Document")) 'Удалён свой обработчик из списка открытия PDF'
  Assert-Check (!(Test-Path -LiteralPath $shortcut)) 'Удалён ярлык меню Пуск'
  Assert-Check (!(Test-Path -LiteralPath $desktopShortcut)) 'Удалён ярлык рабочего стола'
  Assert-Check (!(Test-Path -LiteralPath $settingsShortcut)) 'Удалён ярлык настроек PDF'
  Assert-Check (!(Test-Path -LiteralPath $exe)) 'Удалён EXE приложения'
  Assert-Check ((Get-FileHash -LiteralPath $document).Hash -eq $documentHash) 'Собственный PDF пользователя сохранён при удалении'
  Assert-Check ((Read-Choice) -eq $choiceBefore) 'Удаление сохранило текущий выбор PDF по умолчанию'
  Assert-Check ((Read-Value 'HKCU:\Software\Classes\Applications\AstraPDF.exe\shell\open\command') -eq $productionCommandBefore) 'Удаление тестовой версии не затронуло рабочую'
  $completed = $true
} finally {
  @{ passed = $completed; checks = $checks.ToArray(); installer_sha256 = (Get-FileHash -LiteralPath $installerPath).Hash; os = [Environment]::OSVersion.Version.ToString(); output = $output } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath "$output\results.json" -Encoding UTF8
  Write-Output "Результаты: $output\results.json"
}
Write-Output "Пройдено проверок: $($checks.Count)"
