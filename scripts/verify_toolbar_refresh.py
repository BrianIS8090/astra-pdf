"""Доступные кнопки после открытия PDF сразу рисуются доступными, без наведения."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from pypdf import PdfReader
from verify_ui import Viewer, send, u, wait


def verify(exe, output, theme):
  output.mkdir(parents=True, exist_ok=False)
  previous = os.environ.get('LOCALAPPDATA')
  os.environ['LOCALAPPDATA'] = str(output / 'profile')
  profile = output / 'profile' / 'AstraPDF'
  profile.mkdir(parents=True)
  (profile / 'theme.txt').write_text(theme, encoding='utf-8')
  viewer = None
  try:
    demo = output / 'demo.pdf'
    subprocess.run([str(exe), '--demo', str(demo)], check=True)
    original = demo.read_bytes()
    document = output / 'working.pdf'
    document.write_bytes(original)
    viewer = Viewer(exe, document, output / 'window.json')
    viewer.ready()
    wait(lambda: viewer.snapshot()['thumbnail_count'] >= 2)
    time.sleep(.2)
    before = viewer.snapshot()
    assert before['theme'] == theme
    assert 'button_paints' in before, 'Требуется сборка с наблюдением естественной отрисовки --ui-test'
    # Публичные кнопки: печать, навигация, масштаб и поиск; отключённые исключаются.
    identifiers = [11, 12, 13, 14, 15, 16, 17, 18, 70]
    paints = before['button_paints']
    disabled = {identifier: paints[identifier] for identifier in ['12', '58']}
    assert all(not row['enabled'] and row['drawn_disabled'] for row in disabled.values()), \
      'Предыдущая на первой странице и сохранение без правок должны оставаться недоступными'
    enabled = [str(identifier) for identifier in identifiers
      if paints.get(str(identifier), {}).get('enabled')]
    assert enabled, 'Не найдены доступные кнопки панели'
    for identifier in enabled:
      button = u.GetDlgItem(viewer.hwnd, int(identifier))
      send(button, 0x200, 0, 0x00080008)
      wait(lambda: viewer.snapshot()['button_paints'][identifier]['count'] > paints[identifier]['count'])
      send(button, 0x2a3)
    time.sleep(.1)
    after = viewer.snapshot()
    rows = [{
      'id': int(identifier),
      'enabled': paints[identifier]['enabled'],
      'before': paints[identifier],
      'after': after['button_paints'][identifier]
    } for identifier in enabled]
    failed = [row['id'] for row in rows if row['before']['drawn_disabled']]
    report = {'passed': not failed, 'failed_buttons': failed,
      'capture': 'last_completed_natural_paint; no WM_PRINT, WM_PRINTCLIENT or capture_window',
      'buttons': rows, 'disabled_controls': disabled}
    (output / 'toolbar-refresh.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    print(json.dumps({'theme': theme, 'passed': not failed, 'failed_buttons': failed}, ensure_ascii=False), flush=True)
    assert not failed, f'Доступные кнопки остались тусклыми до наведения: {failed}'
    assert all(row['before']['foreground'] == row['after']['foreground'] for row in rows), \
      'Наведение изменило базовый цвет доступной кнопки'
    report['passed'] = False
    checks = ['load_ready_without_hover', 'hover_keeps_enabled_foreground',
      'disabled_previous_and_clean_save_stay_dim']

    def settled():
      viewer.ready(lambda s: not s['editor']['busy'])
      time.sleep(.15)
      return viewer.snapshot()

    def assert_paints(state):
      observed = {identifier: state['button_paints'][identifier]
        for identifier in [str(i) for i in identifiers] + ['58']}
      report.setdefault('transitions', []).append(observed)
      (output / 'toolbar-refresh.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
      assert all(row['drawn_disabled'] != row['enabled'] for row in observed.values()), \
        'Отрисовка кнопок не соответствует их фактической доступности'
      return state

    viewer.command(10)
    viewer.dismiss(2)
    assert_paints(settled())
    checks.append('cancel_open_dialog_keeps_toolbar_state')
    viewer.open(document)
    reopened = assert_paints(settled())
    assert reopened['pages'] == 3 and not reopened['editor']['dirty']
    checks.append('open_dialog_reload_ready_without_hover')

    generation = reopened['generation']
    viewer.command(91)
    def busy_state():
      state = viewer.snapshot()
      return state if state['editor']['busy'] else None
    busy = wait(busy_state)
    assert not busy['button_paints']['11']['enabled'], 'Во время операции печать недоступна'
    edited = assert_paints(viewer.ready(lambda s: s['generation'] > generation and not s['editor']['busy']))
    # Дублирование одной страницы в известном трёхстраничном примере даёт четыре.
    assert edited['pages'] == 4 and edited['editor']['dirty']
    assert edited['button_paints']['58']['enabled'] and not edited['button_paints']['58']['drawn_disabled']
    assert document.read_bytes() == original
    checks.append('busy_to_ready_and_dirty_save_enabled_without_hover')
    viewer.command(58)
    wait(lambda: not viewer.snapshot()['editor']['dirty'])
    saved = assert_paints(settled())
    assert not saved['button_paints']['58']['enabled'] and saved['button_paints']['58']['drawn_disabled']
    assert len(PdfReader(document).pages) == 4 and demo.read_bytes() == original
    checks.append('save_returns_clean_button_to_disabled_paint')
    report['checks'] = checks
    report['theme'] = theme
    report['passed'] = True
    (output / 'toolbar-refresh.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    print(json.dumps({'theme': theme, 'passed': checks}, ensure_ascii=False), flush=True)
  finally:
    if viewer:
      if viewer.process.poll() is None:
        u.PostMessageW(viewer.hwnd, 0x10, 0, 0)
        try:
          viewer.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
          viewer.process.terminate()
          viewer.process.wait(timeout=10)
    if previous is None:
      os.environ.pop('LOCALAPPDATA', None)
    else:
      os.environ['LOCALAPPDATA'] = previous


if __name__ == '__main__':
  exe, output = (Path(arg).resolve() for arg in sys.argv[1:])
  output.mkdir(parents=True, exist_ok=False)
  for theme in ['light', 'dark']:
    verify(exe, output / theme, theme)
