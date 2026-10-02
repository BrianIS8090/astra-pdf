import hashlib
import json
from pathlib import Path
import sys
import time
from pypdf import PdfWriter
from pypdf.generic import DecodedStreamObject, DictionaryObject, NameObject
from PIL import Image
from verify_ui import Viewer, send, u, windows, c, w


def drawing(path):
  writer = PdfWriter()
  page = writer.add_blank_page(width=2384, height=1684)
  font = writer._add_object(DictionaryObject({NameObject('/Type'): NameObject('/Font'), NameObject('/Subtype'): NameObject('/Type1'), NameObject('/BaseFont'): NameObject('/Helvetica')}))
  page[NameObject('/Resources')] = DictionaryObject({NameObject('/Font'): DictionaryObject({NameObject('/F1'): font})})
  commands = ['0 0 0 RG 0.01 w']
  for i in range(3001):
    x = 1080 + i * .08
    commands.append(f'{x:.3f} 730 m {x:.3f} 970 l S')
  commands += ['0.1 w 50 50 2284 1584 re S', 'BT /F1 0.3 Tf 1191 842 Td (VECTOR DETAIL 0123456789) Tj ET']
  stream = DecodedStreamObject()
  stream.set_data('\n'.join(commands).encode('ascii'))
  page[NameObject('/Contents')] = writer._add_object(stream)
  with path.open('wb') as output:
    writer.write(output)


def run(exe, output, documents):
  output.mkdir(parents=True, exist_ok=False)
  synthetic = output / 'drawing.pdf'
  drawing(synthetic)
  report = {'checks': [], 'documents': []}
  for index, document in enumerate([synthetic] + documents):
    fingerprint = hashlib.sha256(document.read_bytes()).hexdigest()
    viewer = Viewer(exe, document, output / f'document-{index}.json')
    try:
      start = viewer.ready()
      assert not start['detail_mode']
      measurements = {'document': 'synthetic' if index == 0 else f'external-{index}', 'fast_first_frame_ms': start['first_frame_ms'], 'zoom': []}
      mode = u.GetDlgItem(viewer.hwnd, 28)
      send(mode, 0x14e, 1)
      viewer.command(28)
      assert viewer.ready()['detail_mode']
      canvas = next(v['hwnd'] for v in windows(viewer.pid, viewer.hwnd) if v['class'] == 'AstraPdfCanvas')
      for target in [1., 4., 16., 64.]:
        begun = time.perf_counter()
        while viewer.snapshot()['scale'] < target - .001:
          viewer.command(15)
        immediate = viewer.snapshot()
        assert immediate['preview_available'], 'Во время увеличения пропал обзор страницы'
        state = viewer.ready()
        elapsed = (time.perf_counter() - begun) * 1000
        assert state['scale'] >= target - .001 and not state['error']
        _, _, width, height = next(p for p in state['page_boxes'] if p[0] == state['page'])
        if width * height <= 8_000_000:
          # Небольшая страница уже рисуется целиком в нужном разрешении.
          assert state['image_size'] == [width, height] and state['tiles_visible'] == 0
        else:
          assert state['tiles_visible'] > 0 and state['tiles_ready'] == state['tiles_visible']
        assert state['tile_bytes'] <= 64 * 1024 * 1024
        measurements['zoom'].append({'scale': state['scale'], 'burst_and_detail_ms': round(elapsed), 'tiles': state['tiles_ready'], 'tile_bytes': state['tile_bytes']})
        if target == 64.:
          send(viewer.hwnd, 0x8009)
          if index == 0:
            parent, child = w.RECT(), w.RECT()
            u.GetWindowRect(viewer.hwnd, c.byref(parent))
            u.GetWindowRect(canvas, c.byref(child))
            image = Image.open(viewer.output.with_suffix('.png')).convert('RGB')
            x = child.left-parent.left + state['viewport'][0]//2
            y = child.top-parent.top + state['viewport'][1]//2
            values = [image.getpixel((px, y))[0] for px in range(x-256, x+256)]
            transitions = sum((a < 150) != (b < 150) for a,b in zip(values, values[1:]))
            assert min(values) < 60 and max(values) > 230 and transitions > 40, 'Микролинии не различимы в фактическом окне'
            measurements['visible_microline_transitions'] = transitions
      before = viewer.snapshot()['scroll']
      begun = time.perf_counter()
      send(canvas, 0x114, 3)
      pan = viewer.ready()
      assert pan['scroll'][0] > before[0] and pan['tiles_ready'] == pan['tiles_visible']
      measurements['new_area_ms'] = round((time.perf_counter()-begun)*1000)
      begun = time.perf_counter()
      send(canvas, 0x114, 2)
      assert viewer.ready()['scroll'] == before
      measurements['cached_area_ms'] = round((time.perf_counter()-begun)*1000)
      viewer.command(18)
      rotated = viewer.ready()
      assert rotated['rotation'] == 1 and rotated['tiles_ready'] == rotated['tiles_visible']
      send(mode, 0x14e, 0)
      viewer.command(28)
      fast = viewer.ready()
      assert not fast['detail_mode'] and fast['tiles_visible'] == 0
      assert fast['scale'] <= 8.
      measurements['peak_working_set_bytes'] = viewer.peak_ws
      report['documents'].append(measurements)
      assert hashlib.sha256(document.read_bytes()).hexdigest() == fingerprint
    finally:
      viewer.close()
  report['checks'] = ['fast_mode_default', 'mode_switch_both_directions', 'zoom_100_to_6400_percent', 'preview_preserved', 'all_visible_tiles_complete', 'microline_pixels_visible', 'tile_memory_bounded', 'pan_and_cache_reuse', 'rotation', 'source_unchanged']
  report['success'] = True
  (output / 'detail-verification.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
  print(json.dumps(report, ensure_ascii=False), flush=True)


if __name__ == '__main__':
  run(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve(), [Path(p).resolve() for p in sys.argv[3:]])
