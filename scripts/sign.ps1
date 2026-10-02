param([Parameter(Mandatory=$true)][string]$Path, [switch]$Required)
$ErrorActionPreference = 'Stop'
$target = (Resolve-Path -LiteralPath $Path).Path
$thumbprint = $env:ASTRA_SIGN_CERT_THUMBPRINT
if (!$thumbprint) {
  if ($Required) { throw 'Для подписи задайте ASTRA_SIGN_CERT_THUMBPRINT сертификата издателя в хранилище Windows.' }
  return
}
if ($thumbprint -notmatch '^[a-fA-F0-9]{40}$') { throw 'Некорректный отпечаток сертификата.' }
$certificate = Get-Item -LiteralPath "Cert:\CurrentUser\My\$thumbprint"
if (!$certificate.HasPrivateKey -or $certificate.NotAfter -lt (Get-Date)) { throw 'Сертификат просрочен или недоступен его закрытый ключ.' }
$purposes = @($certificate.Extensions | Where-Object { $_ -is [Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension] } | ForEach-Object { $_.EnhancedKeyUsages } | ForEach-Object { $_.Value })
if ($purposes -notcontains '1.3.6.1.5.5.7.3.3') { throw 'Нужен сертификат подписи программ.' }
$tool = $env:ASTRA_SIGNTOOL
if (!$tool) {
  $candidates = @(Get-ChildItem -Path "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue | Sort-Object FullName -Descending)
  if (!$candidates.Count) { throw 'Не найден signtool.exe из Windows SDK. Задайте ASTRA_SIGNTOOL.' }
  $tool = $candidates[0].FullName
}
$timestamp = if ($env:ASTRA_TIMESTAMP_URL) { $env:ASTRA_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }
& $tool sign /sha1 $thumbprint /s My /fd SHA256 /tr $timestamp /td SHA256 $target
if ($LASTEXITCODE -ne 0) { throw 'Подпись программы не выполнена.' }
$signature = Get-AuthenticodeSignature -LiteralPath $target
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Thumbprint -ne $thumbprint) { throw 'Проверка подписи издателя не пройдена.' }
Write-Output "Подпись проверена: $target"
