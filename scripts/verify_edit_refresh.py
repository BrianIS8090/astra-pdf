import json
import os
from pathlib import Path
import sys
import time
import ctypes as c
import pdfplumber
from pypdf import PdfReader
from verify_ui import Viewer, send, set_text, windows, wait, u


def verify(exe, source, output):
  output.mkdir(parents=True, exist_ok=False)
  original = source.read_bytes()
  document = output / 'working.pdf'
  document.write_bytes(original)
  profile = os.environ.get('LOCALAPPDATA')
  os.environ['LOCALAPPDATA'] = str(output / 'profile')
  checks = []
  viewer = None
  u.UpdateWindow.argtypes = [c.c_void_p]
  try:
    viewer = Viewer(exe, document, output / 'window.json')
    viewer.ready()
    canvas = next(r['hwnd'] for r in windows(parent=viewer.hwnd) if r['class'] == 'AstraPdfCanvas')
    viewer.command(120)
    viewer.command(16)
    viewer.ready()
    wait(lambda: viewer.snapshot()['thumbnail_count'] >= min(2, len(PdfReader(source).pages)))

    def point(p):
      s = viewer.snapshot()
      _, x, y = next(r for r in s['page_origins'] if r[0] == 0)
      _, _, width, height = next(r for r in s['page_boxes'] if r[0] == 0)
      return (int(x + p[0] * width) & 65535) | ((int(y + p[1] * height) & 65535) << 16)

    def drag(a, b):
      send(canvas, 0x201, 1, point(a))
      send(canvas, 0x200, 1, point(b))
      send(canvas, 0x202, 0, point(b))

    def click(p):
      lp = point(p)
      send(canvas, 0x201, 1, lp)
      send(canvas, 0x202, 0, lp)

    def monitor(label, action):
      before = viewer.ready()
      u.UpdateWindow(canvas)
      before = viewer.snapshot()
      list_window = u.GetDlgItem(viewer.hwnd, 20)
      count = send(list_window, 0x18b)
      top = send(list_window, 0x18e)
      action()
      samples = []
      deadline = time.monotonic() + 45
      while time.monotonic() < deadline:
        u.UpdateWindow(canvas)
        s = viewer.snapshot()
        samples.append({k: s[k] for k in ['generation', 'rendering', 'refreshing_revision', 'placeholder_paints', 'display_ready_pages', 'page_render_counts']})
        assert s['pages'] == before['pages'], 'Число страниц не должно сбрасываться при локальной правке'
        assert s['scroll'] == before['scroll'] and s['scale'] == before['scale'], 'Правка сместила документ'
        assert s['page_boxes'] == before['page_boxes'], 'Правка перестроила геометрию страниц'
        assert s['display_ready_pages'] == before['display_ready_pages'], 'Исчезло готовое изображение'
        assert s['placeholder_paints'] == before['placeholder_paints'], 'При правке нарисована заглушка'
        assert s['page_render_counts'][1:] == before['page_render_counts'][1:], 'Повторно отрисованы соседние страницы'
        assert send(list_window, 0x18b) == count and send(list_window, 0x18e) == top, 'Сброшен список миниатюр'
        if s['generation'] > before['generation'] and not s['rendering'] and not s['refreshing_revision'] and not s['editor']['busy']:
          break
        time.sleep(.01)
      else:
        raise TimeoutError('Правка не завершилась')
      assert s['page_render_counts'][0] > before['page_render_counts'][0], 'Изменённая страница не обновилась'
      assert document.read_bytes() == original, 'Локальная правка записана без команды сохранения'
      checks.append({'name': label, 'samples': samples, 'changed_pixels': s['image_hash'] != before['image_hash']})
      print(label, flush=True)
      return s

    def accept_comment():
      dialog = wait(lambda: next((r for r in windows(viewer.pid) if r['visible'] and r['text'] == 'Новый комментарий'), None))
      children = windows(parent=dialog['hwnd'])
      set_text(next(r['hwnd'] for r in children if r['class'] == 'Edit'), 'Проверка обновления одной страницы')
      button = next(r['hwnd'] for r in children if r['class'] == 'Button' and r['id'] == 1)
      u.PostMessageW(button, 0xf5, 0, 0)

    for command, label in [(123, 'arrow'), (122, 'rectangle'), (124, 'highlight')]:
      viewer.command(command)
      monitor(label, lambda: drag((.2, .4), (.4, .5)))
    viewer.command(121)
    click((.5, .5))
    monitor('comment', accept_comment)
    monitor('undo_comment', lambda: viewer.command(47))
    monitor('redo_comment', lambda: viewer.command(60))

    viewer.command(23)
    with pdfplumber.open(source) as pdf:
      words = pdf.pages[0].extract_words()
      word = next((w for w in words if w['text'] == 'Типовой'), None)
      if word:
        p = pdf.pages[0]
        text_point = ((word['x0'] + word['x1']) / 2 / p.width, (word['top'] + word['bottom']) / 2 / p.height)
      else:
        text_point = None
    if text_point:
      viewer.command(41)
      wait(lambda: viewer.snapshot()['editor']['mode'] == 'Select')
      click(text_point)
      wait(lambda: viewer.snapshot()['editor']['selected_object'] and not viewer.snapshot()['editor']['busy'])
      viewer.command(42)
      wait(lambda: viewer.snapshot()['draft'])
      field = next(r['hwnd'] for r in windows(parent=canvas) if r['id'] == 140)
      set_text(field, 'Типовой этаж этаж')
      wait(lambda: viewer.snapshot()['draft']['preview'])
      monitor('text_apply', lambda: viewer.command(141))
      monitor('undo_text', lambda: viewer.command(47))
      monitor('redo_text', lambda: viewer.command(60))
      viewer.command(23)

    # Детальные участки должны заменяться без белого кадра и возврата старых деталей.
    mode = u.GetDlgItem(viewer.hwnd, 28)
    send(mode, 0x14e, 1)
    viewer.command(28)
    viewer.ready()
    for _ in range(8):
      viewer.command(15)
    viewer.ready()
    viewer.command(123)
    s = viewer.snapshot()
    _, x, y = next(r for r in s['page_origins'] if r[0] == 0)
    _, _, width, height = next(r for r in s['page_boxes'] if r[0] == 0)
    a = ((s['viewport'][0] * .4 - x) / width, (s['viewport'][1] * .4 - y) / height)
    b = ((s['viewport'][0] * .6 - x) / width, (s['viewport'][1] * .6 - y) / height)
    monitor('detail_arrow', lambda: drag(a, b))
    monitor('detail_undo', lambda: viewer.command(47))
    monitor('detail_redo', lambda: viewer.command(60))
    viewer.command(23)
    send(viewer.hwnd, 0x8009)
    viewer.command(58)
    wait(lambda: not viewer.snapshot()['editor']['dirty'] and not viewer.snapshot()['editor']['busy'])
    assert len(PdfReader(document).pages[0]['/Annots']) >= 5
    viewer.close()
    viewer = None
    assert source.read_bytes() == original
  finally:
    if viewer and viewer.process.poll() is None:
      viewer.process.kill()
      viewer.process.wait(timeout=10)
    if profile is None:
      os.environ.pop('LOCALAPPDATA', None)
    else:
      os.environ['LOCALAPPDATA'] = profile
    (output / 'results.json').write_text(json.dumps({'checks': checks, 'source_unchanged': source.read_bytes() == original}, ensure_ascii=False, indent=2), 'utf-8')


if __name__ == '__main__':
  verify(*(Path(p).resolve() for p in sys.argv[1:4]))
