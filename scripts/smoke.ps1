param([string]$Python = 'python')
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\verify.ps1" -Python $Python
