import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

from pypdf import PdfReader, PdfWriter
import verify_ui as ui


def check_pages(result, baseline):
  printed = PdfReader(result)
  expected = PdfReader(baseline)
  assert len(expected.pages) == 3, 'Контрольный документ должен содержать три страницы'
  assert len(printed.pages) == 2, 'Из системного диалога должны напечататься страницы 2–3'
  for actual, reference in zip(printed.pages, expected.pages[1:]):
    assert len(actual.images) == 1 and len(reference.images) == 1
    a = actual.images[0].image.convert('RGB')
    b = reference.images[0].image.convert('RGB')
    assert a.size == b.size and a.tobytes() == b.tobytes(), 'Содержимое или порядок страниц диапазона неверны'
  return {'page_count': 2, 'page_numbers': [2, 3], 'pixels_match': True}


def powershell(*arguments):
  executable = Path(os.environ['SystemRoot']) / 'System32/WindowsPowerShell/v1.0/powershell.exe'
  completed = subprocess.run([str(executable), '-NoProfile', '-ExecutionPolicy', 'Bypass', *arguments], capture_output=True, timeout=35)
  if completed.returncode:
    detail = completed.stderr.decode('utf-8', errors='replace')
    raise RuntimeError(f'Системная проверка не выполнена (код {completed.returncode}): {detail}')
  return completed.stdout


def assert_desktop_available():
  powershell('-Command', "Add-Type -AssemblyName UIAutomationClient; $f=[System.Windows.Automation.AutomationElement]::FocusedElement; if (!$f -or $f.Current.ProcessId -le 0) { throw 'Не удалось проверить активную сессию Windows' }; $p=Get-Process -Id $f.Current.ProcessId; if ($p.ProcessName -in @('LockApp','LogonUI')) { throw 'Разблокируйте Windows для проверки системного диапазона печати' }")


def run(verified):
  assert_desktop_available()
  summary = json.loads((verified / 'verification.json').read_text('utf-8-sig'))
  assert summary['automated_checks_passed']
  executable = verified / 'AstraPDF/AstraPDF.exe'
  assert hashlib.sha256(executable.read_bytes()).hexdigest().upper() == summary['executable_sha256']
  output = verified / ('native-print-' + time.strftime('%Y%m%d-%H%M%S'))
  output.mkdir()
  demo = verified / 'Слои и страницы.pdf'
  source_hash = hashlib.sha256(demo.read_bytes()).hexdigest()
  result_pdf = output / 'native-range-2-3.pdf'
  old_dll = os.environ.pop('ASTRA_PDFIUM_PATH', None)
  viewer = None
  printer = None
  try:
    viewer = ui.Viewer(executable, demo, output / 'window.json')
    viewer.ready(lambda state: state['pages'] == 3)
    viewer.command(11)
    viewer.dialog()
    helper = Path(__file__).with_name('print-dialog.ps1').resolve()
    powershell('-File', str(helper), '-Action', 'Range', '-PageRange', '2-3', '-Output', str(output / 'range-controls.json'))
    # Нажатие печати может ждать закрытия следующего системного диалога.
    shell = Path(os.environ['SystemRoot']) / 'System32/WindowsPowerShell/v1.0/powershell.exe'
    printer = subprocess.Popen([str(shell), '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(helper), '-Action', 'Print'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    dialog = ui.wait(lambda: next((window for window in ui.windows(viewer.pid) if window['class'] == '#32770' and window['visible'] and 'Сохран' in window['text']), None))
    children = ui.windows(parent=dialog['hwnd'])
    (output / 'save-dialog.json').write_text(json.dumps(children, ensure_ascii=False, indent=2), encoding='utf-8')
    edits = [child for child in children if child['class'] == 'Edit' and child['visible']]
    assert len(edits) == 1, 'Неоднозначное поле имени файла в диалоге сохранения печати'
    ui.set_text(edits[0]['hwnd'], str(result_pdf))
    save = next(child for child in children if child['class'] == 'Button' and child['id'] == 1)
    ui.u.PostMessageW(save['hwnd'], 0xf5, 0, 0)
    ui.wait(lambda: viewer.snapshot()['print_result'] is not None)
    state = viewer.snapshot()
    assert state['print_result'] is True and not state['printing'] and not state['error']
    assert printer.wait(timeout=10) == 0
    checks = check_pages(result_pdf, verified / 'printed.pdf')
    assert hashlib.sha256(demo.read_bytes()).hexdigest() == source_hash
    evidence = {**checks, 'passed': True, 'executable_sha256': summary['executable_sha256'], 'source_sha256': source_hash, 'pdf_sha256': hashlib.sha256(result_pdf.read_bytes()).hexdigest(), 'output': str(result_pdf), 'completed_at': time.strftime('%Y-%m-%dT%H:%M:%S%z')}
    evidence_file = output / 'acceptance.json'
    evidence_file.write_text(json.dumps(evidence, ensure_ascii=False, indent=2), encoding='utf-8')
    summary['native_print_range_acceptance'] = 'passed'
    summary['native_print_evidence'] = str(evidence_file)
    (verified / 'verification.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2), encoding='utf-8')
    print(str(evidence_file))
  finally:
    try:
      if viewer is not None:
        viewer.close()
    finally:
      if printer is not None and printer.poll() is None:
        printer.terminate()
        printer.wait(timeout=10)
      if old_dll is not None:
        os.environ['ASTRA_PDFIUM_PATH'] = old_dll


def self_test(verified, output):
  output.mkdir()
  baseline = verified / 'printed.pdf'
  source = PdfReader(baseline)
  for name, indices, accepted in [('correct', [1, 2], True), ('wrong-count', [1], False), ('wrong-pages', [0, 1], False), ('reversed', [2, 1], False)]:
    writer = PdfWriter()
    for index in indices:
      writer.add_page(source.pages[index])
    target = output / (name + '.pdf')
    with target.open('wb') as file:
      writer.write(file)
    try:
      check_pages(target, baseline)
      result = True
    except AssertionError:
      result = False
    assert result == accepted, name
  print('Проверка напечатанного диапазона: 4 сценария пройдены')


if __name__ == '__main__':
  verified = Path(sys.argv[1]).resolve()
  if len(sys.argv) == 4 and sys.argv[2] == '--self-test':
    self_test(verified, Path(sys.argv[3]).resolve())
  else:
    run(verified)
