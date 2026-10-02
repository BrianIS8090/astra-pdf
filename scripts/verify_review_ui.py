import json
import os
from pathlib import Path
import sys
import time
from pypdf import PdfReader
from verify_ui import Viewer, send, set_text, windows, wait, u


def verify(exe,source,output):
  output.mkdir(parents=True,exist_ok=False)
  original=source.read_bytes()
  document=output/'working.pdf'
  document.write_bytes(original)
  profile=os.environ.get('LOCALAPPDATA')
  os.environ['LOCALAPPDATA']=str(output/'profile')
  v=None
  checks=[]
  def passed(label):
    checks.append(label)
    print(label,flush=True)
  def state():return v.snapshot()
  def dialog(title):return wait(lambda:next((r for r in windows(v.pid) if r['visible'] and r['text']==title),None))
  def accept(title,text):
    d=dialog(title)
    field=next(r['hwnd'] for r in windows(parent=d['hwnd']) if r['class']=='Edit')
    set_text(field,text)
    button=next(r['hwnd'] for r in windows(parent=d['hwnd']) if r['id']==1 and r['class']=='Button')
    u.PostMessageW(button,0xf5,0,0)
  def point(p):
    s=state()
    _,x,y=next(r for r in s['page_origins'] if r[0]==0)
    _,_,width,height=next(r for r in s['page_boxes'] if r[0]==0)
    return (int(x+p[0]*width)&65535)|((int(y+p[1]*height)&65535)<<16)
  def click(p):
    send(canvas,0x201,1,point(p))
    send(canvas,0x202,0,point(p))
  def drag(a,b):
    send(canvas,0x201,1,point(a))
    send(canvas,0x200,1,point(b))
    send(canvas,0x202,0,point(b))
  def changed(g):return v.ready(lambda s:s['generation']>g and not s['editor']['busy'])
  try:
    v=Viewer(exe,document,output/'window.json')
    v.ready()
    canvas=next(r['hwnd'] for r in windows(parent=v.hwnd) if r['class']=='AstraPdfCanvas')
    v.command(120)
    wait(lambda:state()['review']['open'])
    time.sleep(.3)
    v.ready()
    v.command(121)
    g=state()['generation']
    click((.2,.2))
    accept('Новый комментарий','Проверить мощность светильника')
    changed(g)
    assert state()['editor']['document_dirty'] and document.read_bytes()==original
    passed('comment_is_an_unsaved_pdf_edit_with_no_new_user_file')
    for command,a,b in [(122,(.2,.4),(.4,.6)),(123,(.3,.3),(.5,.5)),(124,(.3,.65),(.7,.7))]:
      v.command(command)
      g=state()['generation']
      drag(a,b)
      changed(g)
    v.command(127)
    accept('Масштаб чертежа','100')
    wait(lambda:not state()['dialog_open'])
    assert state()['review']['scale']==100
    v.command(125)
    g=state()['generation']
    drag((.2,.75),(.45,.75))
    changed(g)
    v.command(126)
    for p in [(.6,.3),(.8,.3),(.8,.5),(.6,.5)]:click(p)
    assert len(state()['review']['points'])==4
    g=state()['generation']
    u.PostMessageW(canvas,0x100,13,0)
    changed(g)
    v.command(23)
    v.command(58)
    wait(lambda:not state()['editor']['busy'] and not state()['editor']['dirty'])
    reader=PdfReader(document)
    notes=[a.get_object() for a in reader.pages[0]['/Annots']]
    assert [n['/Subtype'] for n in notes]==['/Text','/Square','/Line','/Highlight','/Line','/Polygon']
    assert notes[0]['/Contents']=='Проверить мощность светильника'
    assert notes[4]['/AstraScale']==100 and notes[5]['/AstraScale']==100
    assert ' m' in notes[4]['/Contents'] and 'm2' in notes[5]['/Contents']
    assert all('/AP' in n and n['/F']==4 for n in notes)
    passed('six_annotation_tools_save_standard_pdf_appearances_and_measurements')
    send(v.hwnd,0x8009)
    time.sleep(.2)
    (output/'review.png').write_bytes(v.output.with_suffix('.png').read_bytes())
    wait(lambda:0 in state()['reader']['loaded_pages'])
    r=notes[0]['/Rect']
    box=reader.pages[0].mediabox
    comment_point=((float(r[0])+float(r[2]))*.5/float(box.width),1-(float(r[1])+float(r[3]))*.5/float(box.height))
    click(comment_point)
    wait(lambda:state()['review']['selected'] is not None)
    v.command(128)
    g=state()['generation']
    accept('Комментарий','Мощность проверена')
    changed(g)
    v.command(58)
    wait(lambda:not state()['editor']['dirty'] and not state()['editor']['busy'])
    assert PdfReader(document).pages[0]['/Annots'][0].get_object()['/Contents']=='Мощность проверена'
    passed('existing_comment_can_be_selected_edited_and_saved')
    wait(lambda:0 in state()['reader']['loaded_pages'])
    click(comment_point)
    wait(lambda:state()['review']['selected'] is not None)
    g=state()['generation']
    v.command(129)
    changed(g)
    g=state()['generation']
    v.command(47)
    changed(g)
    assert not state()['editor']['dirty']
    passed('delete_comment_and_undo_restore_saved_document_exactly')
    v.close()
    v=None
  finally:
    if v and v.process.poll() is None:
      v.process.kill()
      v.process.wait(timeout=10)
    if profile is None:os.environ.pop('LOCALAPPDATA',None)
    else:os.environ['LOCALAPPDATA']=profile
    (output/'results.json').write_text(json.dumps({'checks':checks,'source_unchanged':source.read_bytes()==original},ensure_ascii=False,indent=2),encoding='utf-8')


if __name__=='__main__':verify(*(Path(p).resolve() for p in sys.argv[1:4]))
