import hashlib
import json
from pathlib import Path
import subprocess
import sys

from pypdf import PdfReader


def verify(exe, output):
  subprocess.run([str(exe), '--editing-selftest', str(output)], check=True, timeout=90, creationflags=subprocess.CREATE_NO_WINDOW)
  source = PdfReader(output / 'source.pdf', strict=True)
  pages = PdfReader(output / 'pages-1-3.pdf', strict=True)
  assert len(pages.pages) == 2
  assert [p.extract_text() for p in pages.pages] == [source.pages[i].extract_text() for i in (0, 2)]
  assert len(pages.trailer['/Root']['/OCProperties']['/OCGs']) == 3
  edited = PdfReader(output / 'edited.pdf', strict=True)
  assert 'Проверка текста 123' in edited.pages[0].extract_text().replace('\xa0', ' ')
  objects = json.loads((output / 'objects.json').read_text(encoding='utf-8'))
  selected = next(o for o in objects if o['kind'] == 1 and o['text'])
  deleted = PdfReader(output / 'deleted.pdf', strict=True)
  assert selected['text'] not in deleted.pages[0].extract_text()
  assert len(deleted.pages) == len(source.pages) == len(edited.pages)
  cleaned = PdfReader(output / 'clean.pdf', strict=True)
  assert len(cleaned.pages) == len(source.pages)
  assert set(cleaned.trailer['/Root'].keys()) == {'/Type', '/Pages'}
  assert set(cleaned.trailer.keys()) == {'/Size', '/Root'}
  image_count = 0
  for before, after in zip(source.pages, cleaned.pages):
    assert list(before.mediabox) == list(after.mediabox)
    assert not after.extract_text()
    assert set(after.keys()) == {'/Type', '/Parent', '/MediaBox', '/Resources', '/Contents'}
    assert set(after['/Resources'].keys()) == {'/XObject'}
    for obj in after['/Resources']['/XObject'].values():
      obj = obj.get_object()
      assert obj['/Subtype'] == '/Image'
      assert obj['/ColorSpace'] == '/DeviceRGB'
      assert len(obj.get_data()) == obj['/Width'] * obj['/Height'] * 3
      image_count += 1
  original = (output / 'source.pdf').read_bytes()
  pixelated = PdfReader(output / 'pixelated.pdf', strict=True)
  assert not pixelated.pages[0].extract_text()
  grid = json.loads((output / 'pixel-grid.json').read_text())
  assert len({tuple(grid['bgra'][i:i+3]) for i in range(0, len(grid['bgra']), 4)}) > 10
  first = next(iter(pixelated.pages[0]['/Resources']['/XObject'].values())).get_object()
  data = first.get_data()
  for y in range(0, first['/Height'], 13):
    for x in range(0, first['/Width'], 17):
      gx, gy = x * grid['width'] // 2500, y * grid['height'] // 3334
      i = (gy * grid['width'] + gx) * 4
      j = (y * first['/Width'] + x) * 3
      assert data[j:j+3] == bytes(reversed(grid['bgra'][i:i+3]))
  assert (output / 'restored.pdf').read_bytes() == original
  encrypted = (output / 'fixture.astravault').read_bytes()
  assert original not in encrypted
  assert (output / 'fixture.astrakey').stat().st_size == 40
  result = {'passed': True, 'checks': ['page_content_and_order', 'layers_retained', 'cyrillic_text_reopened', 'object_deleted', 'clean_pdf_image_only', 'no_original_metadata_or_attachments', 'page_dimensions', 'exact_original_restored', 'content_pixelation_matches_preview_grid'], 'image_tiles': image_count, 'source_sha256': hashlib.sha256(original).hexdigest()}
  (output / 'independent-verification.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
  print(json.dumps(result, ensure_ascii=True))


if __name__ == '__main__':
  verify(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve())
