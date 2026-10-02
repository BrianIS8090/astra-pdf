$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$projectRoot = Split-Path -Parent $PSScriptRoot
$toolsDir = Join-Path $projectRoot 'tools'
$compilerDir = Join-Path $toolsDir 'inno-7.1.0'
$download = Join-Path $toolsDir 'innosetup-7.1.0-x64.exe'
$expected = '0362a383ed217d4c4239b5933866dd96d3eb2102737da92f80f6057a4b40df2f'
New-Item -ItemType Directory -Force -Path $toolsDir | Out-Null
if (!(Test-Path -LiteralPath $download)) {
  Invoke-WebRequest -UseBasicParsing 'https://github.com/jrsoftware/issrc/releases/download/is-7_1_0/innosetup-7.1.0-x64.exe' -OutFile $download
}
if ((Get-FileHash -LiteralPath $download -Algorithm SHA256).Hash -ne $expected) {
  throw 'Контрольная сумма Inno Setup не совпала'
}
$signature = Get-AuthenticodeSignature -LiteralPath $download
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'CN=Pyrsys B\.V\.') {
  throw 'Не подтверждена подпись издателя Inno Setup'
}
if (!(Test-Path -LiteralPath "$compilerDir\ISCC.exe")) {
  $process = Start-Process -FilePath $download -ArgumentList @('/CURRENTUSER', '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', '/NOICONS', '/TASKS="!fileassoc"', "/DIR=`"$compilerDir`"") -WindowStyle Hidden -PassThru -Wait
  if ($process.ExitCode -ne 0) { throw "Установка Inno Setup завершилась с кодом $($process.ExitCode)" }
}
if (!(Test-Path -LiteralPath "$compilerDir\ISCC.exe")) { throw 'Компилятор установщика не найден' }
Write-Output "Компилятор: $compilerDir\ISCC.exe"
