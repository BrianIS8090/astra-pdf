import ctypes as c
import json
import os
from pathlib import Path
import sys
import time
import pdfplumber
from pypdf import PdfReader
from verify_ui import Viewer, send, set_text, windows, wait, u


def verify(exe, source, output):
  output.mkdir(parents=True, exist_ok=False)
  original = source.read_bytes()
  document = output/'working.pdf'
  document.write_bytes(original)
  profile = os.environ.get('LOCALAPPDATA')
  os.environ['LOCALAPPDATA'] = str(output/'profile')
  checks = []
  v = None
  def passed(s):
    checks.append(s)
    print(s, flush=True)
  try:
    v = Viewer(exe, document, output/'window.json')
    v.ready()
    canvas = next(r['hwnd'] for r in windows(parent=v.hwnd) if r['class']=='AstraPdfCanvas')
    def state():
      return v.snapshot()
    def field(id):
      return next(r['hwnd'] for r in windows(parent=canvas) if r['id']==id)
    with pdfplumber.open(source) as pdf:
      p = pdf.pages[0]
      word = next(w for w in p.extract_words() if w['text']=='Типовой')
      point = ((word['x0']+word['x1'])/2/p.width, (word['top']+word['bottom'])/2/p.height)
    def click(point):
      s = state()
      _, x, y = next(p for p in s['page_origins'] if p[0]==0)
      _, _, w, h = next(p for p in s['page_boxes'] if p[0]==0)
      lp = int(x+point[0]*w) | int(y+point[1]*h)<<16
      send(canvas, 0x201, 1, lp)
      send(canvas, 0x202, 0, lp)
    def select():
      if state()['editor']['mode']!='Select':
        v.command(41)
        wait(lambda:state()['editor']['mode']=='Select')
      click(point)
      wait(lambda:state()['editor']['selected_object'] and not state()['editor']['busy'])
    select()
    v.command(42)
    wait(lambda:state()['draft'])
    set_text(field(140), 'Черновик для отмены')
    v.command(23)
    assert state()['draft'] is None and not state()['editor']['dirty']
    assert document.read_bytes()==original
    passed('inline_cancel_discards_only_draft_without_disk_or_history_change')
    v.command(42)
    wait(lambda:state()['draft'])
    set_text(field(140), 'Несовместимый символ 😀')
    v.command(141)
    wait(lambda:state()['draft'] and state()['draft']['error'])
    assert not state()['editor']['dirty'] and document.read_bytes()==original
    passed('unsupported_glyph_keeps_edit_open_and_preserves_original')
    value = 'Новый заголовок\nВторая строка'
    set_text(field(140), value.replace('\n','\r\n'))
    send(field(145), 0x14e, 1, 0)
    send(canvas, 0x111, 145 | 1<<16, field(145))
    wait(lambda:state()['draft']['preview'])
    assert state()['draft']['font'].lower().endswith('arial.ttf')
    v.command(143)
    wait(lambda:state()['draft']['show_preview'])
    send(v.hwnd, 0x8009)
    time.sleep(.2)
    (output/'preview.png').write_bytes((output/'window.png').read_bytes())
    assert document.read_bytes()==original and not state()['editor']['dirty']
    passed('explicit_font_and_multiline_preview_are_visible_without_committing')
    g = state()['generation']
    v.command(141)
    v.ready(lambda s:s['generation']>g)
    assert state()['draft'] is None and state()['editor']['undo_count']==1
    v.command(58)
    wait(lambda:not state()['editor']['busy'] and not state()['editor']['dirty'])
    result = PdfReader(document).pages[0].extract_text()
    assert all(line in result for line in value.split('\n'))
    passed('apply_is_one_undo_step_and_multiline_font_survives_save_reopen')
    # Движение и изменение размера проверяем на выбранном объекте текущей страницы.
    select()
    bounds = state()['editor']['selected_object']['bounds']
    def drag(a,b):
      s=state()
      _,x,y=next(p for p in s['page_origins'] if p[0]==0)
      _,_,w,h=next(p for p in s['page_boxes'] if p[0]==0)
      lp=lambda p:int(x+p[0]*w) | int(y+p[1]*h)<<16
      g=s['generation']
      send(canvas,0x201,1,lp(a));send(canvas,0x200,1,lp(b));send(canvas,0x202,0,lp(b))
      v.ready(lambda s:s['generation']>g and not s['editor']['busy'])
    center=((bounds[0]+bounds[2])/2,(bounds[1]+bounds[3])/2)
    drag(center,(center[0]+.03,center[1]+.02))
    assert state()['editor']['document_dirty']
    v.command(47)
    v.ready()
    passed('object_drag_changes_session_and_undo_restores_previous_revision')
    select()
    bounds = state()['editor']['selected_object']['bounds']
    drag((bounds[2],bounds[3]), (bounds[2]+.025,bounds[3]+.01))
    assert state()['editor']['document_dirty']
    v.command(47)
    v.ready()
    assert not state()['editor']['document_dirty']
    passed('object_corner_resize_and_undo_restore_saved_revision')
    v.close()
    v=None
  finally:
    if v and v.process.poll() is None:
      v.process.kill()
      v.process.wait(timeout=10)
    if profile is None:
      os.environ.pop('LOCALAPPDATA',None)
    else:
      os.environ['LOCALAPPDATA']=profile
    (output/'results.json').write_text(json.dumps({'checks':checks,'source_unchanged':source.read_bytes()==original},ensure_ascii=False,indent=2),encoding='utf-8')


if __name__=='__main__':
  verify(*(Path(p).resolve() for p in sys.argv[1:4]))
