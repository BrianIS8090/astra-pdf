import ctypes as c
import json
import os
from pathlib import Path
import sys
import time
from pypdf import PdfReader
import pdfplumber
from verify_ui import Viewer, send, set_text, windows, wait, u, w, k, info


def verify(exe, source, output):
  output.mkdir(parents=True, exist_ok=False)
  working = output / 'working.pdf'
  original = source.read_bytes()
  working.write_bytes(original)
  profile = output / 'profile'
  previous_profile = os.environ.get('LOCALAPPDATA')
  os.environ['LOCALAPPDATA'] = str(profile)
  checks = []
  limitations = []
  v = None
  def passed(label):
    checks.append(label)
    print(label, flush=True)
  def state():
    return v.snapshot()
  def dialog(title):
    return wait(lambda: next((r for r in windows(v.pid) if r['visible'] and r['text'] == title), None))
  def button(d, identifier):
    handle = next(r['hwnd'] for r in windows(parent=d['hwnd']) if r['id'] == identifier and r['class'] == 'Button')
    u.PostMessageW(handle, 0xf5, 0, 0)
  def changed(generation):
    return v.ready(lambda s: s['generation'] > generation and not s['editor']['busy'])
  def point(page, p):
    s = state()
    _, x, y = next(r for r in s['page_origins'] if r[0] == page)
    _, _, width, height = next(r for r in s['page_boxes'] if r[0] == page)
    return (int(x+p[0]*width), int(y+p[1]*height))
  def lp(p):
    return (p[0] & 65535) | (p[1] & 65535) << 16
  def drag(a,b):
    send(canvas,0x201,1,lp(a))
    send(canvas,0x200,1,lp(b))
    send(canvas,0x202,0,lp(b))
  def screenshot(name):
    send(v.hwnd,0x8009)
    time.sleep(.2)
    (output/name).write_bytes(v.output.with_suffix('.png').read_bytes())
  try:
    v = Viewer(exe, working, output/'window.json')
    v.ready()
    canvas = next(r['hwnd'] for r in windows(parent=v.hwnd) if r['class'] == 'AstraPdfCanvas')
    v.command(70)
    wait(lambda: state()['reader']['search_open'])
    set_text(u.GetDlgItem(v.hwnd,74),'свет')
    v.command(76)
    wait(lambda: state()['reader']['hits'] > 0 and not state()['reader']['busy'])
    first = state()['reader']['current']
    v.command(76)
    wait(lambda: state()['reader']['current'] != first)
    v.command(75)
    assert state()['reader']['current'] == first
    screenshot('search.png')
    passed('search_cyrillic_real_document_next_previous_and_highlight')
    v.command(77)
    v.page(1)
    v.command(16)
    v.ready()
    wait(lambda: 0 in state()['reader']['loaded_pages'])
    with pdfplumber.open(working) as pdf:
      p = pdf.pages[0]
      words = p.extract_words()
      word = next(t for t in words if len(t['text']) > 4 and t['top']/p.height < .7)
      a = ((word['x0']+.5)/p.width,(word['top']+word['bottom'])/2/p.height)
      b = ((word['x1']-.5)/p.width,a[1])
    v.command(71)
    wait(lambda: state()['reader']['text_mode'])
    drag(point(0,a),point(0,b))
    wait(lambda: state()['reader']['selection'] is not None)
    v.command(80)
    u.OpenClipboard.argtypes=[w.HWND]
    u.GetClipboardData.argtypes=[w.UINT]
    u.GetClipboardData.restype=w.HANDLE
    assert word['text'].strip('.,:') in state()['reader']['selected_text']
    if u.OpenClipboard(None):
      try:
        handle=u.GetClipboardData(13)
        pointer=k.GlobalLock(handle)
        copied=c.wstring_at(pointer)
        k.GlobalUnlock(handle)
      finally:
        u.CloseClipboard()
      assert word['text'].strip('.,:') in copied, (word,copied)
      passed('system_clipboard_contains_selected_text')
    else:
      limitations.append(f'system_clipboard_unavailable_win32_error_{c.get_last_error()}')
    screenshot('selection.png')
    v.command(23)
    assert not state()['reader']['text_mode']
    passed('text_selection_and_deactivation_in_real_window')
    v.command(52)
    wait(lambda: state()['editor']['mode']=='Mask')
    drag(point(0,(.2,.3)),point(0,(.6,.5)))
    wait(lambda: len(state()['editor']['masks'])==1)
    v.command(23)
    v.command(47)
    wait(lambda: not state()['editor']['masks'])
    assert state()['editor']['redo_count']==1
    v.command(60)
    wait(lambda: len(state()['editor']['masks'])==1)
    before_pages=state()['pages']
    generation=state()['generation']
    v.command(91)
    changed(generation)
    assert state()['pages']==before_pages+1
    assert [m['page'] for m in state()['editor']['masks']]==[0,1]
    generation=state()['generation']
    v.command(47)
    changed(generation)
    assert state()['pages']==before_pages and len(state()['editor']['masks'])==1
    generation=state()['generation']
    v.command(60)
    changed(generation)
    assert state()['pages']==before_pages+1 and len(state()['editor']['masks'])==2
    passed('duplicate_page_undo_redo_preserve_mask_mapping')
    wait(lambda: state()['editor']['recovery_saved'])
    passed('mask_undo_redo_and_encrypted_checkpoint_in_live_window')
    assert working.read_bytes()==original
    v.process.kill()
    v.process.wait(timeout=10)
    wait(lambda:not list((profile/'AstraPDF'/'Sessions').glob('*.pdf')))
    passed('windows_removes_plaintext_working_revisions_after_crash')
    v = Viewer(exe,working,output/'recovered.json')
    d = dialog('Восстановление сеанса')
    button(d,6)
    v.ready(lambda s:len(s['editor']['masks'])==2 and s['pages']==before_pages+1 and not s['editor']['busy'])
    assert working.read_bytes()==original
    assert state()['editor']['dirty']
    passed('real_process_crash_recovers_pdf_and_masks_without_changing_original')
    v.command(47)
    wait(lambda: len(state()['editor']['masks'])==1)
    v.command(47)
    wait(lambda: not state()['editor']['masks'])
    assert state()['editor']['document_dirty']
    u.PostMessageW(v.hwnd,0x10,0,0)
    d=dialog('Несохранённые изменения')
    button(d,7)
    v.process.wait(timeout=15)
    v.close()
    v = None
    assert not list((profile/'AstraPDF'/'Recovery').glob('*/state.dat'))
    passed('normal_close_discards_recovery_after_discarding_changes')
  finally:
    if v and v.process.poll() is None:
      v.process.kill()
      v.process.wait(timeout=10)
    if previous_profile is None:
      os.environ.pop('LOCALAPPDATA',None)
    else:
      os.environ['LOCALAPPDATA']=previous_profile
    (output/'results.json').write_text(json.dumps({'checks':checks,'limitations':limitations,'source_unchanged':source.read_bytes()==original},ensure_ascii=False,indent=2),encoding='utf-8')


if __name__=='__main__':
  verify(*(Path(p).resolve() for p in sys.argv[1:4]))
