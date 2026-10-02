import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys
import time
import pdfplumber
from pypdf import PdfReader
from verify_ui import Viewer, send, set_text, windows, wait, u, w, k, info, alive


def verify(exe, source, output):
  output.mkdir(parents=True, exist_ok=False)
  original = source.read_bytes()
  working = output / 'working.pdf'
  working.write_bytes(original)
  profile = output / 'profile'
  old_profile = os.environ.get('LOCALAPPDATA')
  os.environ['LOCALAPPDATA'] = str(profile)
  try:
    v = Viewer(exe, working, output / 'window.json')
  finally:
    if old_profile is None:
      os.environ.pop('LOCALAPPDATA', None)
    else:
      os.environ['LOCALAPPDATA'] = old_profile
  checks = []
  def passed(name):
    checks.append(name)
    print(name, flush=True)
  def state():
    return v.snapshot()['editor']
  def dialog(title):
    return wait(lambda: next((r for r in windows(v.pid) if r['visible'] and r['text'] == title), None))
  def button(d, identifier):
    handle = next(r['hwnd'] for r in windows(parent=d['hwnd']) if r['id'] == identifier and r['class'] == 'Button')
    u.PostMessageW(handle, 0xf5, 0, 0)
  def save_as(path=None):
    shortcut(0x53, shift=True)
    d = dialog('Сохранить как')
    if path is None:
      button(d, 2)
    else:
      field = next(r['hwnd'] for r in windows(parent=d['hwnd']) if r['class'] == 'Edit' and r['visible'])
      set_text(field, str(path))
      button(d, 1)
    wait(lambda: not v.snapshot()['dialog_open'] and not state()['busy'])
  def settle_change(generation):
    wait(lambda: not state()['busy'] and v.snapshot()['generation'] > generation)
    return v.ready()
  def undo():
    g = v.snapshot()['generation']
    shortcut(0x5a)
    settle_change(g)
  def shortcut(key, shift=False):
    thread = u.GetWindowThreadProcessId(v.hwnd, None)
    assert u.AttachThreadInput(k.GetCurrentThreadId(), thread, True)
    keys = (c.c_ubyte * 256)()
    u.GetKeyboardState(keys)
    saved = bytes(keys)
    try:
      keys[17] = 128
      keys[16] = 128 if shift else 0
      assert u.SetKeyboardState(keys)
      u.PostMessageW(v.hwnd, 0x100, key, 0)
      time.sleep(.15)
    finally:
      u.SetKeyboardState((c.c_ubyte * 256).from_buffer_copy(saved))
      u.AttachThreadInput(k.GetCurrentThreadId(), thread, False)
  try:
    v.ready()
    canvas = next(r['hwnd'] for r in windows(parent=v.hwnd) if r['class'] == 'AstraPdfCanvas')
    with pdfplumber.open(source) as pdf:
      p = pdf.pages[0]
      words = p.extract_words()
      word = next((q for q in words if q['text'] == 'Типовой'), words[0])
      point = ((word['x0']+word['x1'])/2/p.width, (word['top']+word['bottom'])/2/p.height)
    def select():
      if state()['mode'] != 'Select':
        v.command(41)
        wait(lambda: state()['mode'] == 'Select')
      s = v.snapshot()
      _, ox, oy = next(p for p in s['page_origins'] if p[0] == 0)
      _, _, width, height = next(p for p in s['page_boxes'] if p[0] == 0)
      lp = int(ox+point[0]*width) | int(oy+point[1]*height) << 16
      send(canvas, 0x201, 1, lp)
      send(canvas, 0x202, 0, lp)
      wait(lambda: not state()['busy'] and state()['selected_object'] is not None)
      assert state()['selected_object']['kind'] == 1
    def change(text):
      select()
      g = v.snapshot()['generation']
      v.command(42)
      d = dialog('Изменить текст')
      field = next(r['hwnd'] for r in windows(parent=d['hwnd']) if r['class'] == 'Edit')
      set_text(field, text)
      button(d, 1)
      settle_change(g)
    change('Session edit ONE')
    assert state()['document_dirty'] and info(v.hwnd)['text'].startswith('* working.pdf')
    assert working.read_bytes() == original and list(output.glob('*.pdf')) == [working]
    assert not v.snapshot()['dialog_open']
    passed('text_edit_stays_in_session_without_save_dialog_or_user_copy')
    # Последовательные действия отменяются в обратном порядке.
    select()
    g = v.snapshot()['generation']
    v.command(43)
    settle_change(g)
    assert state()['undo_count'] == 2 and working.read_bytes() == original
    undo()
    assert state()['document_dirty']
    undo()
    assert not state()['dirty'] and not info(v.hwnd)['text'].startswith('*')
    passed('delete_is_immediate_and_two_undos_restore_original')
    v.command(15)
    before = v.ready()
    change('Session edit TWO')
    after = v.ready()
    assert before['page'] == after['page'] and before['scale'] == after['scale'] and before['rotation'] == after['rotation']
    passed('editing_preserves_page_zoom_and_rotation')
    save_as()
    assert state()['document_dirty'] and working.read_bytes() == original
    passed('cancel_save_as_keeps_unsaved_work')
    shortcut(0x53)
    wait(lambda: not state()['busy'] and not state()['dirty'])
    assert 'Session edit TWO' in PdfReader(working).pages[0].extract_text().replace('\xa0', ' ')
    assert not info(v.hwnd)['text'].startswith('*')
    passed('save_updates_current_file_and_clears_dirty_marker')
    undo()
    assert state()['dirty'] and 'Session edit TWO' in PdfReader(working).pages[0].extract_text().replace('\xa0', ' ')
    copied = output / 'save-as.pdf'
    save_as(copied)
    assert copied.read_bytes() == original and info(v.hwnd)['text'].startswith('save-as.pdf')
    assert not state()['dirty']
    passed('undo_after_save_and_save_as_change_target_without_touching_previous_file')
    change('Session edit THREE')
    # Внешняя правка не должна затереться.
    copied.write_bytes(original + b'\n% changed outside\n')
    v.command(58)
    d = dialog('Astra PDF')
    assert state()['dirty'] and copied.read_bytes().endswith(b'% changed outside\n')
    u.PostMessageW(d['hwnd'], 0x10, 0, 0)
    wait(lambda: not v.snapshot()['dialog_open'])
    passed('external_change_rejects_overwrite_and_keeps_edits')
    copied.write_bytes(original)
    # Windows может запретить замену открытого другим приложением файла.
    k.CreateFileW.argtypes = [w.LPCWSTR, w.DWORD, w.DWORD, c.c_void_p, w.DWORD, w.DWORD, w.HANDLE]
    k.CreateFileW.restype = w.HANDLE
    lock = k.CreateFileW(str(copied), 0x80000000, 1, None, 3, 0, None)
    assert lock not in (None, c.c_void_p(-1).value)
    try:
      v.command(58)
      d = dialog('Astra PDF')
      assert state()['dirty'] and copied.read_bytes() == original
      u.PostMessageW(d['hwnd'], 0x10, 0, 0)
      wait(lambda: not v.snapshot()['dialog_open'])
    finally:
      k.CloseHandle(lock)
    passed('write_failure_keeps_disk_and_unsaved_session')
    u.PostMessageW(v.hwnd, 0x10, 0, 0)
    button(dialog('Несохранённые изменения'), 2)
    wait(lambda: not v.snapshot()['dialog_open'])
    assert alive(v.pid) and state()['dirty']
    passed('cancel_close_keeps_session')
    # Открытие другого документа также предлагает сохранить изменения.
    v.command(10)
    d = v.dialog()
    field = next(r['hwnd'] for r in windows(parent=d['hwnd']) if r['class'] == 'Edit' and r['visible'])
    set_text(field, str(working))
    button(d, 1)
    button(dialog('Несохранённые изменения'), 2)
    wait(lambda: not v.snapshot()['dialog_open'])
    assert state()['dirty'] and 'save-as.pdf' in info(v.hwnd)['text']
    passed('cancel_open_keeps_current_document_and_edits')
    for width in [1120, 2200]:
      u.MoveWindow(v.hwnd, 30, 30, width, 1000, True)
      v.ready()
      send(v.hwnd, 0x8009)
      shutil.copyfile(v.output.with_suffix('.png'), output / f'ui-{width}.png')
    u.PostMessageW(v.hwnd, 0x10, 0, 0)
    button(dialog('Несохранённые изменения'), 6)
    wait(lambda: not alive(v.pid))
    assert 'Session edit THREE' in PdfReader(copied).pages[0].extract_text().replace('\xa0', ' ')
    assert not list((profile / 'AstraPDF' / 'Sessions').glob('*.pdf'))
    passed('save_on_close_and_session_snapshot_cleanup')
    assert source.read_bytes() == original
    v.process.wait(timeout=5)
    k.CloseHandle(v.handle)
    os.environ['LOCALAPPDATA'] = str(profile)
    try:
      v = Viewer(exe, working, output / 'lifecycle.json')
    finally:
      if old_profile is None:
        os.environ.pop('LOCALAPPDATA', None)
      else:
        os.environ['LOCALAPPDATA'] = old_profile
    v.ready()
    canvas = next(r['hwnd'] for r in windows(parent=v.hwnd) if r['class'] == 'AstraPdfCanvas')
    change('Saved on open')
    def open_answer(path, answer):
      v.command(10)
      d = v.dialog()
      field = next(r['hwnd'] for r in windows(parent=d['hwnd']) if r['class'] == 'Edit' and r['visible'])
      set_text(field, str(path))
      button(d, 1)
      button(dialog('Несохранённые изменения'), answer)
      wait(lambda: not v.snapshot()['dialog_open'] and not state()['busy'] and not state()['dirty'])
      v.ready()
    open_answer(copied, 6)
    assert 'Saved on open' in PdfReader(working).pages[0].extract_text().replace('\xa0', ' ')
    assert 'save-as.pdf' in info(v.hwnd)['text']
    passed('save_then_open_another_document')
    change('Discarded on open')
    open_answer(working, 7)
    assert 'Discarded' not in PdfReader(copied).pages[0].extract_text()
    passed('discard_then_open_another_document')
    change('Discarded on close')
    u.PostMessageW(v.hwnd, 0x10, 0, 0)
    button(dialog('Несохранённые изменения'), 7)
    wait(lambda: not alive(v.pid))
    assert 'Discarded' not in PdfReader(working).pages[0].extract_text()
    assert not list((profile / 'AstraPDF' / 'Sessions').glob('*.pdf'))
    passed('discard_on_close_preserves_last_saved_file')
    result = {'passed': True, 'checks': checks, 'original_sha256': hashlib.sha256(original).hexdigest()}
    (output / 'results.json').write_text(json.dumps(result, indent=2), 'utf-8')
  finally:
    if alive(v.pid):
      try:
        (output / 'failure.json').write_text(json.dumps(v.snapshot(), indent=2), 'utf-8')
        send(v.hwnd, 0x8009)
        for d in windows(v.pid):
          if d['visible'] and d['class'] in ['AstraPdfDialog', '#32770']:
            u.PostMessageW(d['hwnd'], 0x10, 0, 0)
        wait(lambda: not state()['busy'])
        dirty = state()['dirty']
        u.PostMessageW(v.hwnd, 0x10, 0, 0)
        if dirty:
          button(dialog('Несохранённые изменения'), 7)
        wait(lambda: not alive(v.pid))
      except Exception:
        v.close()


if __name__ == '__main__':
  verify(*(Path(x).resolve() for x in sys.argv[1:]))
