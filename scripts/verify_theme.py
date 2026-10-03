"""Проверка темы в настоящем окне: цвета, состояние PDF, сохранение выбора."""
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import sys
from PIL import Image
from verify_ui import Viewer, send, windows, wait, u, w


def verify(exe, document, output):
  output.mkdir(parents=True, exist_ok=False)
  previous = os.environ.get('LOCALAPPDATA')
  os.environ['LOCALAPPDATA'] = str(output / 'profile')
  original = hashlib.sha256(document.read_bytes()).hexdigest()
  checks = []
  viewer = None
  try:
    viewer = Viewer(exe, document, output / 'window.json')
    viewer.ready()
    wait(lambda: viewer.snapshot()['thumbnail_count'] >= min(2, viewer.snapshot()['pages']))
    viewer.command(15)
    viewer.ready()
    viewer.command(44)
    wait(lambda: viewer.snapshot()['editor']['ready'] > 0)
    before = viewer.snapshot()
    keys = ['page', 'scale', 'scroll', 'rotation', 'generation', 'ticket', 'image_hash',
      'states', 'page_boxes', 'page_render_counts', 'thumbnail_count', 'editor', 'reader']
    u.GetClientRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
    u.ClientToScreen.argtypes = [w.HWND, c.POINTER(w.POINT)]

    def screenshot(name, theme):
      send(viewer.hwnd, 0x8009)
      path = output / name
      path.write_bytes(viewer.output.with_suffix('.png').read_bytes())
      image = Image.open(path).convert('RGB')
      root = w.RECT()
      u.GetWindowRect(viewer.hwnd, c.byref(root))
      point = w.POINT(0, 0)
      u.ClientToScreen(viewer.hwnd, c.byref(point))
      dpi = viewer.snapshot()['dpi'] / 96
      x, y = point.x-root.left+int(4*dpi), point.y-root.top+int(6*dpi)
      expected = (33, 40, 50) if theme == 'dark' else (248, 249, 250)
      actual = image.getpixel((x, y))
      assert actual == expected, (name, actual, expected, (x, y))

    for expected in ['dark', 'light', 'dark']:
      viewer.command(29)
      after = viewer.snapshot()
      assert after['theme'] == expected
      for key in keys:
        assert after[key] == before[key], ('Тема изменила документ или сеанс', key, before[key], after[key])
      screenshot(f'{expected}.png', expected)
      checks.append(f'Тема {expected}: оформление изменено без перерисовки PDF и сброса сеанса')
    # Геометрия переключателя остаётся доступной при минимальной ширине окна.
    for width in [560, 980, 1400]:
      scale = before['dpi'] / 96
      u.MoveWindow(viewer.hwnd, 30, 30, int(width*scale), int(720*scale), True)
      viewer.ready()
      children = windows(parent=viewer.hwnd)
      button = next(r['hwnd'] for r in children if r['id'] == 29)
      bounds = w.RECT()
      u.GetWindowRect(button, c.byref(bounds))
      outer = w.RECT()
      u.GetWindowRect(viewer.hwnd, c.byref(outer))
      assert outer.left <= bounds.left < bounds.right <= outer.right
      for child in children:
        if child['class'] != 'Button' or not child['visible'] or child['id'] == 29:
          continue
        rect = w.RECT()
        u.GetWindowRect(child['hwnd'], c.byref(rect))
        overlap = min(bounds.right, rect.right) > max(bounds.left, rect.left) and min(bounds.bottom, rect.bottom) > max(bounds.top, rect.top)
        assert not overlap, ('Кнопки пересекаются', width, child)
    checks.append('Переключатель доступен без наложения кнопок при ширине 560/980/1400')
    viewer.command(23)
    viewer.close()
    viewer = Viewer(exe, document, output / 'restart.json')
    assert viewer.ready()['theme'] == 'dark'
    checks.append('После перезапуска восстановлена тёмная тема')
    viewer.command(29)
    viewer.close()
    viewer = Viewer(exe, document, output / 'restart-light.json')
    assert viewer.ready()['theme'] == 'light'
    checks.append('После перезапуска восстановлена светлая тема')
    viewer.close()
    viewer = None
    assert hashlib.sha256(document.read_bytes()).hexdigest() == original
    checks.append('Исходный PDF не изменился')
    (output / 'theme-verification.json').write_text(json.dumps({'passed': checks}, ensure_ascii=False, indent=2), encoding='utf-8')
    print(json.dumps(checks, ensure_ascii=False), flush=True)
  finally:
    if viewer and viewer.process.poll() is None:
      viewer.process.terminate()
      viewer.process.wait(10)
    if previous is None:
      os.environ.pop('LOCALAPPDATA', None)
    else:
      os.environ['LOCALAPPDATA'] = previous


if __name__ == '__main__':
  verify(*(Path(arg).resolve() for arg in sys.argv[1:]))
