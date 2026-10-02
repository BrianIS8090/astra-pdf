import json
import os
from pathlib import Path
import sys
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
  def passed(label):checks.append(label);print(label,flush=True)
  def state():return v.snapshot()
  def changed(g):return v.ready(lambda s:s['generation']>g and not s['editor']['busy'])
  def edit(id):
    g=state()['generation']
    v.command(id)
    changed(g)
  def save():
    v.command(58)
    wait(lambda:not state()['editor']['busy'] and not state()['editor']['dirty'])
  def texts():return [p.extract_text() for p in PdfReader(document).pages]
  def choose_file():
    d=v.dialog()
    field=next(r['hwnd'] for r in windows(parent=d['hwnd']) if r['visible'] and r['class']=='Edit')
    set_text(field,str(source))
    b=next(r['hwnd'] for r in windows(parent=d['hwnd']) if r['id']==1 and r['class']=='Button')
    u.PostMessageW(b,0xf5,0,0)
  try:
    v=Viewer(exe,document,output/'window.json')
    v.ready()
    initial=[p.extract_text() for p in PdfReader(source).pages]
    count=len(initial)
    assert count>=3
    edit(91)
    assert state()['pages']==count+1 and document.read_bytes()==original
    edit(47)
    assert not state()['editor']['dirty']
    passed('duplicate_page_is_unsaved_and_undo_restores_exact_source')
    v.page(2)
    edit(94)
    save()
    assert texts()==[initial[1],initial[0]]+initial[2:]
    edit(90)
    save()
    assert texts()==[initial[0]]+initial[2:]
    passed('reorder_and_delete_update_current_file_on_save_only')
    edit(47)
    edit(47)
    assert state()['pages']==count
    save()
    if state()['layers']:
      v.layer(0,False)
      v.ready(lambda s:not s['states'][0])
    old_layers=state()['layers']
    g=state()['generation']
    v.command(92)
    choose_file()
    d=wait(lambda:next((r for r in windows(v.pid) if r['text']=='Вставить страницы' and r['visible']),None))
    field=next(r['hwnd'] for r in windows(parent=d['hwnd']) if r['class']=='Edit')
    set_text(field,'2')
    b=next(r['hwnd'] for r in windows(parent=d['hwnd']) if r['id']==1 and r['class']=='Button')
    u.PostMessageW(b,0xf5,0,0)
    changed(g)
    assert state()['pages']==count+1
    if old_layers:
      wait(lambda:state()['layers']==old_layers*2)
      assert not state()['states'][0]
    save()
    inserted=texts()
    assert inserted.count(initial[1])==2
    passed('insert_selected_foreign_page_preserves_existing_layer_visibility')
    g=state()['generation']
    v.command(93)
    choose_file()
    changed(g)
    save()
    assert texts()==inserted+initial
    passed('merge_appends_all_pages_and_saves_reopenable_pdf')
    v.close()
    v=None
  finally:
    if v and v.process.poll() is None:v.process.kill();v.process.wait(timeout=10)
    if profile is None:os.environ.pop('LOCALAPPDATA',None)
    else:os.environ['LOCALAPPDATA']=profile
    (output/'results.json').write_text(json.dumps({'checks':checks,'source_unchanged':source.read_bytes()==original},ensure_ascii=False,indent=2),encoding='utf-8')


if __name__=='__main__':verify(*(Path(p).resolve() for p in sys.argv[1:4]))
