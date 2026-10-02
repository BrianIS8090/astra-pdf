import json
from pathlib import Path
import sys
from reportlab.pdfgen import canvas
from verify_ui import Viewer, send, u, windows


def verify(exe, output):
  output.mkdir(parents=True, exist_ok=False)
  source = output / 'landscape.pdf'
  pdf = canvas.Canvas(str(source), pagesize=(1122.52, 793.7))
  for page in range(11):
    pdf.drawString(60, 700, f'Landscape page {page + 1}')
    pdf.rect(50, 50, 1022, 693)
    pdf.showPage()
  pdf.save()
  viewer = Viewer(exe, source, output / 'window.json')
  try:
    viewer.ready()
    for page in [2, 5, 11, 1]:
      viewer.page(page)
    viewer.command(16)
    viewer.ready()
    canvas_window = next(r['hwnd'] for r in windows(parent=viewer.hwnd) if r['class'] == 'AstraPdfCanvas')
    send(canvas_window, 0x115, 3)
    viewer.ready()
    viewer.page(1)
    viewer.command(16)
    viewer.ready()
    mode = u.GetDlgItem(viewer.hwnd, 28)
    send(mode, 0x14e, 1)
    viewer.command(28)
    viewer.ready()
    for target in [1., 4., 16., 64.]:
      while viewer.snapshot()['scale'] < target - .001:
        viewer.command(15)
      viewer.ready()
    send(mode, 0x14e, 0)
    viewer.command(28)
    viewer.ready()
    viewer.page(1)
    viewer.command(16)
    final = viewer.ready()
    assert not final['rendering'] and len(final['visible_pages']) >= 2
    assert final['frame_bytes'] <= 96 * 1024 * 1024
    (output / 'results.json').write_text(json.dumps({'passed': True, 'checks': ['return_from_6400_to_fit_with_two_visible_pages'], 'final': final}, indent=2), 'utf-8')
    print('return_from_6400_to_fit_with_two_visible_pages', flush=True)
  finally:
    viewer.close()


if __name__ == '__main__':
  verify(*(Path(x).resolve() for x in sys.argv[1:]))
