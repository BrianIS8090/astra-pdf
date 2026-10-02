import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

import pdfplumber
from pypdf import PdfReader
from reportlab.pdfgen import canvas
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from verify_ui import Viewer, send, set_text, windows, wait, u, w


def fixture(path):
  for name, file in [('Bold', 'arialbd.ttf'), ('Italic', 'timesi.ttf')]:
    pdfmetrics.registerFont(TTFont(name, str(Path(os.environ['WINDIR']) / 'Fonts' / file)))
  pdf = canvas.Canvas(str(path), pagesize=(600, 800))
  pdf.setFont('Bold', 24)
  pdf.setFillColorRGB(.12, .2, .4)
  pdf.drawString(50, 700, 'Типовой этаж 123')
  pdf.setFont('Italic', 18)
  pdf.drawString(50, 610, 'Курсив и форматирование')
  text = pdf.beginText(50, 510)
  text.setFont('Courier-BoldOblique', 16)
  text.setCharSpace(2)
  text.setWordSpace(4)
  text.setHorizScale(85)
  text.textOut('OLD WORD')
  text.setCharSpace(0)
  text.setWordSpace(0)
  text.textOut('KEEP')
  pdf.drawText(text)
  pdf.setFont('Helvetica', 10)
  pdf.drawString(50, 80, 'UNCHANGED NEIGHBOR')
  pdf.save()


def verify(exe, source, output, word='Типовой', new='Типовой этаж 321', page=0):
  output.mkdir(parents=True, exist_ok=False)
  original = source.read_bytes()
  working = output / 'working.pdf'
  working.write_bytes(original)
  with pdfplumber.open(source) as pdf:
    p = pdf.pages[page]
    target = next(q for q in p.extract_words() if q['text'] == word)
    point = ((target['x0'] + target['x1']) / 2 / p.width, (target['top'] + target['bottom']) / 2 / p.height)
    font = next(q for q in p.chars if target['x0'] <= q['x0'] < target['x1'] and abs(q['top']-target['top']) < 2)['fontname']
    original_chars = p.chars
  v = Viewer(exe, working, output / 'window.json')
  checks = []
  old = ''
  try:
    v.ready()
    if page:
      v.page(page+1)
    canvas_hwnd = next(q['hwnd'] for q in windows(parent=v.hwnd) if q['class'] == 'AstraPdfCanvas')
    def state():
      return v.snapshot()['editor']
    def select():
      v.command(41)
      s = v.ready()
      _, ox, oy = next(q for q in s['page_origins'] if q[0] == page)
      _, _, width, height = next(q for q in s['page_boxes'] if q[0] == page)
      lp = int(ox+point[0]*width) | int(oy+point[1]*height) << 16
      send(canvas_hwnd, 0x201, 1, lp)
      send(canvas_hwnd, 0x202, 0, lp)
      wait(lambda: not state()['busy'] and state()['selected_object'] is not None)
    def change(value):
      nonlocal old
      v.command(42)
      d = wait(lambda: next((q for q in windows(v.pid) if q['visible'] and q['text'] == 'Изменить текст'), None))
      field = next(q for q in windows(parent=d['hwnd']) if q['class'] == 'Edit')
      edit = field['hwnd']
      if not old:
        old = field['text']
      set_text(edit, value)
      button = next(q['hwnd'] for q in windows(parent=d['hwnd']) if q['class'] == 'Button' and q['id'] == 1)
      u.PostMessageW(button, 0xf5, 0, 0)
      wait(lambda: not u.IsWindowVisible(d['hwnd']))
      wait(lambda: not state()['busy'])
    select()
    started = time.monotonic()
    change(new)
    if v.snapshot()['dialog_open']:
      messages = [q['text'] for d in windows(v.pid) if d['visible'] and d['class'] in ['#32770', 'AstraPdfDialog'] for q in windows(parent=d['hwnd'])]
      raise AssertionError(messages)
    v.ready()
    elapsed = time.monotonic() - started
    assert state()['document_dirty'] and working.read_bytes() == original
    send(v.hwnd, 0x8009)
    checks.append('same_font_edit_in_session')
    v.command(58)
    wait(lambda: not state()['busy'] and not state()['dirty'])
    after = PdfReader(working, strict=True)
    shutil.copyfile(working, output / 'edited.pdf')
    before = PdfReader(source, strict=True)
    fonts_before = before.pages[page]['/Resources']['/Font']
    fonts_after = after.pages[page]['/Resources']['/Font']
    assert set(fonts_before) == set(fonts_after)
    def canonical(value):
      value = value.get_object() if hasattr(value, 'get_object') else value
      if isinstance(value, dict):
        result = {str(k): canonical(v) for k,v in value.items() if k not in ['/Length', '/Filter', '/DecodeParms']}
        if hasattr(value, 'get_data'):
          result['decoded_hash'] = hashlib.sha256(value.get_data()).hexdigest()
        return result
      if isinstance(value, list):
        return [canonical(v) for v in value]
      return str(value)
    for key in fonts_before:
      a, b = fonts_before[key].get_object(), fonts_after[key].get_object()
      assert canonical(a) == canonical(b)
      desc = a.get('/FontDescriptor')
      if desc:
        for kind in ['/FontFile', '/FontFile2', '/FontFile3']:
          if kind in desc.get_object():
            assert desc.get_object()[kind].get_data() == b['/FontDescriptor'].get_object()[kind].get_data()
    with pdfplumber.open(working) as pdf:
      p = pdf.pages[page]
      assert new in p.extract_text().replace('\xa0', ' ')
      changed_chars = [q for q in p.chars if abs(q['top']-target['top']) < 2 and q['x0'] >= target['x0']-1]
      assert changed_chars and all(q['fontname'] == font for q in changed_chars)
      first = next(q for q in original_chars if target['x0'] <= q['x0'] < target['x1'] and abs(q['top']-target['top']) < 2)
      assert all(abs(q['size']-first['size']) < .005 and q['non_stroking_color'] == first['non_stroking_color'] for q in changed_chars)
      before_neighbors = [(q['text'], q['fontname'], q['size'], q['x0'], q['top']) for q in original_chars if abs(q['top']-target['top']) > 2]
      after_neighbors = [(q['text'], q['fontname'], q['size'], q['x0'], q['top']) for q in p.chars if abs(q['top']-target['top']) > 2]
      assert len(before_neighbors) == len(after_neighbors)
      for a,b in zip(before_neighbors, after_neighbors):
        assert a[:2] == b[:2] and max(abs(x-y) for x,y in zip(a[2:],b[2:])) < .005
    checks += ['font_resources_and_embedded_data_unchanged', 'independent_text_font_and_neighbor_positions']
    # Неподдерживаемый символ не должен создавать историю или менять PDF.
    v.command(50)
    select()
    saved = working.read_bytes()
    undo_count = state()['undo_count']
    change(new + ' 漢')
    d = wait(lambda: next((q for q in windows(v.pid) if q['visible'] and q['class'] == 'AstraPdfDialog'), None))
    assert not state()['document_dirty'] and state()['undo_count'] == undo_count and working.read_bytes() == saved
    u.PostMessageW(d['hwnd'], 0x10, 0, 0)
    wait(lambda: not v.snapshot()['dialog_open'])
    checks.append('missing_glyph_rejected_without_mutation')
    v.command(47)
    wait(lambda: not state()['busy'] and state()['document_dirty'])
    v.ready()
    v.command(58)
    wait(lambda: not state()['busy'] and not state()['dirty'])
    assert working.read_bytes() == original
    checks.append('undo_and_save_restore_exact_original')
    assert source.read_bytes() == original
    result = {'passed': True, 'checks': checks, 'font': font, 'old': old, 'new': new, 'edit_seconds': round(elapsed, 3), 'exe_sha256': hashlib.sha256(exe.read_bytes()).hexdigest(), 'source_sha256': hashlib.sha256(original).hexdigest()}
    (output / 'results.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
    print(json.dumps(result, ensure_ascii=True))
  finally:
    for d in windows(v.pid):
      if d['visible'] and d['class'] in ['#32770', 'AstraPdfDialog']:
        u.PostMessageW(d['hwnd'], 0x10, 0, 0)
    if v.process.poll() is None:
      for _ in range(4):
        if not v.snapshot()['editor']['dirty']:
          break
        v.command(47)
        time.sleep(.2)
      v.close()


if __name__ == '__main__':
  exe, output = (Path(q).resolve() for q in sys.argv[1:3])
  if len(sys.argv) > 3:
    verify(exe, Path(sys.argv[3]).resolve(), output, *sys.argv[4:])
  else:
    output.mkdir(parents=True, exist_ok=False)
    source = output / 'fonts.pdf'
    fixture(source)
    verify(exe, source, output / 'bold')
    verify(exe, source, output / 'italic', 'Курсив', 'Курсив и формат')
