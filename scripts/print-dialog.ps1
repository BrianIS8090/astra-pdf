param([ValidateSet('Inspect', 'Range', 'Print', 'Cancel', 'Properties')][string]$Action = 'Inspect', [string]$Output, [string]$PageRange = '2-3')
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$scope = [System.Windows.Automation.TreeScope]
$element = [System.Windows.Automation.AutomationElement]
$condition = [System.Windows.Automation.PropertyCondition]::new($element::NameProperty, 'Astra PDF — печать')
$window = $element::RootElement.FindFirst($scope::Children, $condition)
if (!$window) { throw 'Не найден системный диалог печати Astra PDF' }
function Find-Control([string]$Id) {
  $condition = [System.Windows.Automation.PropertyCondition]::new($element::AutomationIdProperty, $Id)
  $visible = [System.Windows.Automation.PropertyCondition]::new($element::IsOffscreenProperty, $false)
  $condition = [System.Windows.Automation.AndCondition]::new($condition, $visible)
  $deadline = [DateTime]::UtcNow.AddSeconds(10)
  do {
    $control = $window.FindFirst($scope::Descendants, $condition)
    if ($control) { break }
    Start-Sleep -Milliseconds 100
  } while ([DateTime]::UtcNow -lt $deadline)
  if (!$control) { throw "Не найден элемент печати: $Id" }
  return $control
}
if ($Action -eq 'Range' -or $Action -eq 'Print') {
  $selection = (Find-Control 'printerSelector').GetCurrentPattern([System.Windows.Automation.SelectionPattern]::Pattern).GetCurrentSelection()
  if ($selection.Count -ne 1 -or $selection[0].Current.Name -ne 'Microsoft Print to PDF') {
    throw 'Проверка допускает печать только в Microsoft Print to PDF'
  }
}
if ($Action -eq 'Range') {
  $control = Find-Control 'com.microsoft.JobCustomPageRange_ItemList'
  $pattern = $control.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern)
  $pattern.Expand()
  $nameCondition = [System.Windows.Automation.PropertyCondition]::new($element::NameProperty, 'Настраиваемый диапазон')
  $typeCondition = [System.Windows.Automation.PropertyCondition]::new($element::ControlTypeProperty, [System.Windows.Automation.ControlType]::ListItem)
  $choice = $window.FindFirst($scope::Descendants, [System.Windows.Automation.AndCondition]::new($nameCondition, $typeCondition))
  if (!$choice) { throw 'Не найден выбор диапазона' }
  $choice.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select()
  $edit = Find-Control 'com.microsoft.JobCustomPageRange_ValueText'
  $edit.SetFocus()
  $focused = $element::FocusedElement
  if ($focused.Current.ProcessId -ne $edit.Current.ProcessId -or $focused.Current.AutomationId -ne $edit.Current.AutomationId) {
    throw 'Windows не передала фокус полю диапазона. Разблокируйте рабочий стол и повторите проверку.'
  }
  if ($PageRange -notmatch '^\d+(-\d+)?$') { throw 'Проверка допускает только номер страницы или один числовой диапазон' }
  Add-Type -AssemblyName System.Windows.Forms
  [System.Windows.Forms.SendKeys]::SendWait('^a')
  [System.Windows.Forms.SendKeys]::SendWait($PageRange + '{TAB}')
  $value = $edit.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
  if ($value.Current.Value -ne $PageRange) { throw 'Диалог не сохранил диапазон' }
  (Find-Control 'PrintButton').SetFocus()
}
if ($Action -eq 'Print' -or $Action -eq 'Cancel' -or $Action -eq 'Properties') {
  $id = switch ($Action) { 'Print' { 'PrintButton' } 'Cancel' { 'CloseButton' } 'Properties' { 'moreOptionsButton' } }
  $pattern = (Find-Control $id).GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
  $pattern.Invoke()
  if (!$Output) { return }
}
$items = $window.FindAll($scope::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
$rows = foreach ($item in $items) {
  [pscustomobject]@{
    Name = $item.Current.Name
    Id = $item.Current.AutomationId
    Type = $item.Current.ControlType.ProgrammaticName
    Enabled = $item.Current.IsEnabled
    Offscreen = $item.Current.IsOffscreen
    Value = $(try { $item.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value } catch { $null })
    Patterns = @($item.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName })
  }
}
if ($Output) { $rows | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $Output -Encoding UTF8 }
$rows | ConvertTo-Json -Depth 4
