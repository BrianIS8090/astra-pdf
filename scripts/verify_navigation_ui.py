import ctypes as c
import json
import os
from pathlib import Path
import sys
import time
from reportlab.pdfgen import canvas
from verify_ui import Viewer, send, windows, wait, u, w


def verify(exe, output):
  output.mkdir(parents=True, exist_ok=False)
  source=output/'Navigation.pdf'
  pdf=canvas.Canvas(str(source),pagesize=(600,800))
  pdf.bookmarkPage('first')
  pdf.addOutlineEntry('First chapter','first',level=0)
  pdf.setFont('Helvetica',20)
  pdf.drawString(50,700,'Go to page three')
  pdf.linkRect('', 'third', (50,690,250,725), relative=0, thickness=1)
  pdf.showPage()
  pdf.drawString(50,700,'Second page')
  pdf.showPage()
  pdf.bookmarkPage('third')
  pdf.addOutlineEntry('Third chapter','third',level=0)
  pdf.drawString(50,700,'Third page')
  pdf.save()
  original=source.read_bytes()
  previous={k:os.environ.get(k) for k in ['LOCALAPPDATA','ASTRA_TEST_RECENT']}
  os.environ['LOCALAPPDATA']=str(output/'profile')
  os.environ['ASTRA_TEST_RECENT']='1'
  v=None
  checks=[]
  def passed(s):
    checks.append(s)
    print(s,flush=True)
  try:
    v=Viewer(exe,source,output/'first.json')
    v.ready()
    state=v.snapshot
    canvas_window=next(r['hwnd'] for r in windows(parent=v.hwnd) if r['class']=='AstraPdfCanvas')
    wait(lambda:0 in state()['reader']['loaded_pages'])
    s=state()
    _,x,y=next(p for p in s['page_origins'] if p[0]==0)
    _,_,width,height=next(p for p in s['page_boxes'] if p[0]==0)
    lp=int(x+.2*width) | int(y+.115*height)<<16
    send(canvas_window,0x201,1,lp)
    send(canvas_window,0x202,0,lp)
    v.ready(lambda s:s['page']==2)
    passed('internal_pdf_link_navigates_to_target_page')
    v.command(72)
    wait(lambda:state()['reader']['bookmarks'] is not None)
    bookmarks=u.GetDlgItem(v.hwnd,73)
    assert send(bookmarks,0x18b)==2
    send(bookmarks,0x186,0,0)
    send(v.hwnd,0x111,73 | 1<<16,bookmarks)
    v.ready(lambda s:s['page']==0)
    passed('pdf_bookmark_sidebar_navigates_to_destination')
    v.command(24)
    for dpi in [96,120,144,192]:
      rect=w.RECT(30,30,30+int(760*dpi/96),30+int(670*dpi/96))
      send(v.hwnd,0x2e0,dpi | dpi<<16,c.addressof(rect))
      v.ready(lambda s:s['dpi']==dpi)
      v.command(70)
      time.sleep(.25)
      v.ready()
      send(v.hwnd,0x8009)
      time.sleep(.1)
      (output/f'dpi-{dpi}.png').write_bytes(v.output.with_suffix('.png').read_bytes())
      # У всех видимых кнопок верхней панели есть место внутри окна.
      parent=w.RECT();u.GetClientRect(v.hwnd,c.byref(parent))
      for item in windows(parent=v.hwnd):
        if item['visible'] and item['class']=='Button':
          bounds=w.RECT();u.GetWindowRect(item['hwnd'],c.byref(bounds))
          point=w.POINT(bounds.left,bounds.top);u.ScreenToClient(v.hwnd,c.byref(point))
          assert point.x>=0 and point.x+bounds.right-bounds.left<=parent.right+1,(dpi,item)
      v.command(77)
    passed('toolbar_search_layout_at_100_125_150_200_percent_emulated_dpi')
    v.page(3)
    v.command(15)
    v.command(18)
    before=v.ready()
    wait(lambda:(output/'profile/AstraPDF/reading.dat').exists())
    time.sleep(.65)
    v.close();v=None
    v=Viewer(exe,source,output/'second.json')
    after=v.ready()
    assert (after['page'],after['rotation'])==(before['page'],before['rotation'])
    assert after['scale']==before['scale']
    passed('new_process_resumes_recent_document_page_zoom_and_rotation')
    v.close();v=None
    assert source.read_bytes()==original
    passed('navigation_and_preferences_do_not_modify_pdf')
  finally:
    if v and v.process.poll() is None:
      v.process.kill();v.process.wait(timeout=10)
    for key,value in previous.items():
      if value is None:os.environ.pop(key,None)
      else:os.environ[key]=value
    (output/'results.json').write_text(json.dumps({'checks':checks,'dpi_test':'WM_DPICHANGED; physical monitors not tested'},indent=2),encoding='utf-8')


if __name__=='__main__':
  verify(*(Path(p).resolve() for p in sys.argv[1:3]))
