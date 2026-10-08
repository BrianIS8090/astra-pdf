import ctypes as c
import json
import os
import shutil
from pathlib import Path
import subprocess
import sys
import time
from PIL import Image
from pypdf import PdfReader
from verify_ui import Viewer, send, set_text, windows, wait, u


def verify(exe, source, output):
  output.mkdir(parents=True, exist_ok=False)
  original = source.read_bytes()
  working = output / 'working.pdf'
  working.write_bytes(original)
  previous = os.environ.get('LOCALAPPDATA')
  os.environ['LOCALAPPDATA'] = str(output / 'profile')
  profile = output / 'profile' / 'AstraPDF'
  profile.mkdir(parents=True)
  (profile / 'theme.txt').write_text('dark')
  checks = []
  v = None

  def passed(label):
    checks.append(label)
    print(label, flush=True)

  def state():
    return v.snapshot()

  def dialog(title):
    return wait(lambda: next((r for r in windows(v.pid) if r['visible'] and r['text'] == title), None))

  def button(window, identifier):
    h = next(r['hwnd'] for r in windows(parent=window) if r['class'] == 'Button' and r['id'] == identifier)
    u.PostMessageW(h, 0xf5, 0, 0)

  def accept(title, value):
    d = dialog(title)
    h = next(r['hwnd'] for r in windows(parent=d['hwnd']) if r['class'] == 'Edit' and r['visible'])
    set_text(h, value)
    button(d['hwnd'], 1)

  def point(p):
    s = state()
    _, x, y = next(r for r in s['page_origins'] if r[0] == 0)
    _, _, width, height = next(r for r in s['page_boxes'] if r[0] == 0)
    return (int(x + p[0] * width) & 65535) | ((int(y + p[1] * height) & 65535) << 16)

  def mode(identifier):
    v.command(identifier)
    wait(lambda: state()['review']['mode'] == identifier)

  def color(index, expected):
    h = next(r['hwnd'] for r in windows(parent=v.hwnd) if r['id'] == 133)
    send(h, 0x14e, index, 0)
    send(v.hwnd, 0x111, 133 | 1 << 16, h)
    assert state()['review']['color'] == expected

  def changed(g):
    return v.ready(lambda s: s['generation'] > g and not s['editor']['busy'])

  def save():
    v.command(58)
    wait(lambda: not state()['editor']['busy'] and not state()['editor']['dirty'])

  def export(range_text, name):
    h = next(r['hwnd'] for r in windows(parent=v.hwnd) if r['id'] == 40)
    assert u.IsWindowVisible(h) and u.IsWindowEnabled(h)
    u.PostMessageW(h, 0xf5, 0, 0)
    accept('Сохранить страницы', range_text)
    destination = output / name
    accept('Сохранить выбранные страницы', str(destination))
    wait(lambda: destination.exists() and not state()['editor']['busy'])
    d = dialog('Astra PDF')
    button(d['hwnd'], 1)
    wait(lambda: not state()['dialog_open'])
    return destination

  try:
    v = Viewer(exe, working, output / 'window.json')
    v.ready()
    canvas = next(r['hwnd'] for r in windows(parent=v.hwnd) if r['class'] == 'AstraPdfCanvas')
    v.command(120)
    wait(lambda: state()['review']['open'])
    time.sleep(.2)
    v.ready()
    mode(131)
    color(1, [211, 47, 47])
    g = state()['generation']
    stroke = [(.12, .43), (.17, .40), (.23, .46), (.29, .41), (.35, .46), (.43, .41)]
    send(canvas, 0x201, 1, point(stroke[0]))
    for p in stroke[1:]:
      send(canvas, 0x200, 1, point(p))
    s = state()
    assert len(s['review']['points']) >= 6 and s['generation'] == g
    send(canvas, 0x202, 0, point(stroke[-1]))
    changed(g)
    assert state()['editor']['undo_count'] == 1 and working.read_bytes() == original
    passed('freehand_is_one_unsaved_vector_annotation_with_multiple_points')
    g = state()['generation']
    send(canvas, 0x201, 1, point((.2, .3)))
    send(canvas, 0x200, 1, point((.4, .3)))
    u.PostMessageW(canvas, 0x100, 27, 0)
    wait(lambda: state()['review']['mode'] is None and not state()['review']['points'])
    assert state()['generation'] == g and state()['editor']['undo_count'] == 1
    passed('escape_cancels_unfinished_stroke_and_disables_tool')

    for index, rgb, bounds, text in [
      (0, [26, 107, 194], [(.1, .55), (.7, .72)], 'Цветная надпись\r\nВторая строка'),
      (4, [123, 65, 181], [(.1, .76), (.7, .93)], 'Фиолетовая заметка'),
    ]:
      if state()['review']['mode'] != 132:
        mode(132)
      color(index, rgb)
      g = state()['generation']
      send(canvas, 0x201, 1, point(bounds[0]))
      send(canvas, 0x200, 1, point(bounds[1]))
      send(canvas, 0x202, 0, point(bounds[1]))
      accept('Текстовая заметка', text)
      changed(g)
    assert state()['editor']['undo_count'] == 3 and working.read_bytes() == original
    v.command(23)
    wait(lambda: state()['review']['mode'] is None)
    passed('cyrillic_multiline_text_and_two_colors_stay_in_session_until_save')

    v.command(40)
    d = dialog('Сохранить страницы')
    button(d['hwnd'], 2)
    wait(lambda: not state()['dialog_open'])
    assert working.read_bytes() == original and state()['editor']['dirty']
    chosen = export('1, 3', 'selected.pdf')
    selected = PdfReader(chosen)
    assert len(selected.pages) == 2 and '/OCProperties' in selected.trailer['/Root']
    assert [a.get_object()['/Subtype'] for a in selected.pages[0]['/Annots']] == ['/Ink', '/FreeText', '/FreeText']
    assert state()['editor']['dirty'] and working.read_bytes() == original
    single = export('3', 'single.pdf')
    assert len(PdfReader(single).pages) == 1 and not PdfReader(single).pages[0].get('/Annots')
    passed('page_export_keeps_unsaved_annotations_layers_and_source_without_duplicates')
    save()
    reader = PdfReader(working)
    notes = [a.get_object() for a in reader.pages[0]['/Annots']]
    assert [n['/Subtype'] for n in notes] == ['/Ink', '/FreeText', '/FreeText']
    assert len(notes[0]['/InkList'][0]) >= 12 and all(n['/F'] == 4 and '/AP' in n for n in notes)
    assert 'Вторая строка' in notes[1]['/Contents']
    for n, expected in zip(notes, [[211, 47, 47], [26, 107, 194], [123, 65, 181]]):
      key = '/C' if n['/Subtype'] == '/Ink' else '/AstraTextColor'
      assert all(abs(float(a) - b / 255.) < .001 for a, b in zip(n[key], expected))
    for n in notes[1:]:
      assert n['/C'] == [] and ' rg' in n['/DA']
      fonts = n['/AP']['/N'].get_object()['/Resources']['/Font']
      font = next(iter(fonts.values())).get_object()
      assert '/FontFile2' in font['/DescendantFonts'][0].get_object()['/FontDescriptor'].get_object()
    passed('standard_ink_and_freetext_have_printable_appearances_and_embedded_fonts')

    bundled_poppler = Path(os.environ['USERPROFILE']) / '.cache/codex-runtimes/codex-primary-runtime/dependencies/native/poppler/Library/bin/pdftoppm.exe'
    poppler = Path(os.environ.get('ASTRA_POPPLER') or shutil.which('pdftoppm') or bundled_poppler)
    assert poppler.exists(), 'Для независимой проверки укажите ASTRA_POPPLER — путь к pdftoppm.exe'
    for document, name in [(working, 'external-render'), (chosen, 'external-export-render')]:
      subprocess.run([str(poppler), '-f', '1', '-singlefile', '-r', '72', '-png', str(document), str(output / name)], check=True, capture_output=True)
      image = Image.open(output / (name + '.png')).convert('RGB')
      pixels = image.getcolors(maxcolors=image.width * image.height)
      for color_value in [[211, 47, 47], [26, 107, 194], [123, 65, 181]]:
        count = sum(n for n, p in pixels if all(abs(p[i] - color_value[i]) < 12 for i in range(3)))
        assert count > 30, (name, color_value, count)
    passed('independent_poppler_displays_all_three_annotation_colors_in_document_and_export')
    wait(lambda: 0 in state()['reader']['loaded_pages'])
    send(canvas, 0x201, 1, point((.3, .6)))
    send(canvas, 0x202, 0, point((.3, .6)))
    wait(lambda: state()['review']['selected'] and state()['review']['selected']['kind'] == 3)
    g = state()['generation']
    v.command(128)
    accept('Комментарий', 'Исправленная цветная надпись')
    changed(g)
    save()
    n = PdfReader(working).pages[0]['/Annots'][1].get_object()
    assert n['/Contents'] == 'Исправленная цветная надпись' and n['/AstraTextSize'] == 14
    assert all(abs(float(a) - b / 255.) < .001 for a, b in zip(n['/AstraTextColor'], [26, 107, 194]))
    passed('editing_existing_freetext_preserves_its_color_size_and_embedded_font')
    g = state()['generation']
    v.command(47)
    changed(g)
    g = state()['generation']
    v.command(60)
    changed(g)
    assert not state()['editor']['dirty']
    passed('undo_and_redo_restore_saved_colored_notes')
    v.command(52)
    wait(lambda: state()['editor']['mode'] == 'Mask')
    send(canvas, 0x201, 1, point((.2, .2)))
    send(canvas, 0x200, 1, point((.4, .4)))
    send(canvas, 0x202, 0, point((.4, .4)))
    assert state()['editor']['masks']
    v.command(40)
    d = dialog('Astra PDF')
    field = wait(lambda: next((r['hwnd'] for r in windows(parent=d['hwnd']) if r['class'] == 'Edit'), None))
    message = c.create_unicode_buffer(send(field, 0x0e) + 1)
    send(field, 0x0d, len(message), c.addressof(message))
    assert 'скрытия' in message.value and 'очищенный PDF' in message.value, message.value
    button(d['hwnd'], 1)
    wait(lambda: not state()['dialog_open'])
    v.command(47)
    wait(lambda: not state()['editor']['masks'])
    passed('page_export_with_unapplied_hiding_blocks_is_rejected')
    v.close()
    v = None
    assert source.read_bytes() == original
  finally:
    if v and v.process.poll() is None:
      v.process.kill()
      v.process.wait(timeout=10)
    if previous is None:
      os.environ.pop('LOCALAPPDATA', None)
    else:
      os.environ['LOCALAPPDATA'] = previous
    (output / 'results.json').write_text(json.dumps({'checks': checks, 'source_unchanged': source.read_bytes() == original}, ensure_ascii=False, indent=2), encoding='utf-8')


if __name__ == '__main__':
  verify(*(Path(p).resolve() for p in sys.argv[1:4]))
