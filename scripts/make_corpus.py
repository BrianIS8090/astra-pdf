import hashlib
import json
from pathlib import Path
import random
import sys

from PIL import Image, ImageDraw
from pypdf import PdfReader, PdfWriter
from pypdf.generic import ArrayObject, BooleanObject, DecodedStreamObject, DictionaryObject, NameObject, NumberObject, TextStringObject


def name(value):
  return NameObject('/' + value)


folder = Path(sys.argv[1])
folder.mkdir(parents=True, exist_ok=True)


def document(filename, sizes, layers=0):
  writer = PdfWriter()
  font = writer._add_object(DictionaryObject({name('Type'): name('Font'), name('Subtype'): name('Type1'), name('BaseFont'): name('Helvetica')}))
  groups = []
  for index in range(layers):
    groups.append(writer._add_object(DictionaryObject({name('Type'): name('OCG'), name('Name'): TextStringObject(f'Слой {index + 1:03d}')})))
  if groups:
    config = DictionaryObject({name('BaseState'): name('ON'), name('Order'): ArrayObject(groups), name('OFF'): ArrayObject(groups[1::2])})
    if filename == 'locked-radio.pdf':
      config[name('Locked')] = ArrayObject([groups[0]])
      config[name('RBGroups')] = ArrayObject([ArrayObject(groups[1:3])])
      config[name('OFF')] = ArrayObject([groups[1]])
    writer._root_object[name('OCProperties')] = DictionaryObject({name('OCGs'): ArrayObject(groups), name('D'): config})
  for index, (width, height) in enumerate(sizes):
    page = writer.add_blank_page(width, height)
    page[name('Resources')] = DictionaryObject({name('Font'): DictionaryObject({name('F1'): font}), name('Properties'): DictionaryObject({name(f'L{i}'): group for i, group in enumerate(groups)})})
    content = f'BT /F1 22 Tf 40 {height - 50} Td (PAGE {index + 1} - ASTRA PDF ACCEPTANCE) Tj ET\n'
    for row in range(min(80, int((height - 100) / 14))):
      content += f'BT /F1 10 Tf 40 {height - 90 - row * 14} Td (Document {index + 1}, line {row + 1}: ABC 0123456789 native rendering test.) Tj ET\n'
    for i in range(layers):
      x, y = 50 + i % 15 * 32, 80 + i // 15 * 14
      content += f'/OC /L{i} BDC {i % 3 / 3:.2f} {(i % 5) / 5:.2f} 0.6 rg {x} {y} 25 10 re f EMC\n'
    stream = DecodedStreamObject()
    stream.set_data(content.encode('ascii'))
    page[name('Contents')] = writer._add_object(stream)
  with (folder / filename).open('wb') as output:
    writer.write(output)


document('text-1000.pdf', [(595, 842)] * 1000)
document('mixed-sizes.pdf', [(595, 842), (842, 595), (2384, 3370), (288, 288)])
document('layers-300.pdf', [(595, 842)] * 8, 300)
document('locked-radio.pdf', [(595, 842)] * 3, 3)
image = Image.frombytes('RGB', (2480, 3508), random.Random(20261001).randbytes(2480 * 3508 * 3))
draw = ImageDraw.Draw(image)
draw.rectangle((180, 300, 2300, 2100), fill='white')
for y in range(380, 1900, 80):
  draw.line((260, y, 2200, y), fill=(10, 30, 50), width=7)
image.save(folder / 'scan-8.pdf', 'PDF', resolution=300, save_all=True, append_images=[image] * 7, quality=80)
(folder / 'broken.pdf').write_bytes(b'%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\nINVALID')
(folder / 'empty.pdf').write_bytes(b'')
writer = PdfWriter()
writer.add_blank_page(595, 842)
writer.encrypt('test-password')
with (folder / 'password.pdf').open('wb') as output:
  writer.write(output)

# Известный случай глубокой вложенности: проверяет обработку недоверенного входа.
parts = [b'%PDF-1.7\n']
offsets = [0]
for number, body in enumerate([
  b'<< /Type /Catalog /Pages 2 0 R /X ' + b'[' * 10380 + b']' * 10380 + b' >>',
  b'<< /Type /Pages /Count 1 /Kids [3 0 R] >>',
  b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>',
], 1):
  offsets.append(sum(map(len, parts)))
  parts.append(f'{number} 0 obj\n'.encode() + body + b'\nendobj\n')
xref = sum(map(len, parts))
parts.append(b'xref\n0 4\n0000000000 65535 f \n' + b''.join(f'{offset:010d} 00000 n \n'.encode() for offset in offsets[1:]))
parts.append(f'trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF'.encode())
(folder / 'deep-nesting.pdf').write_bytes(b''.join(parts))

manifest = []
for file in sorted(folder.glob('*.pdf')):
  manifest.append({'file': file.name, 'bytes': file.stat().st_size, 'sha256': hashlib.sha256(file.read_bytes()).hexdigest()})
(folder / 'manifest.json').write_text(json.dumps(manifest, indent=2), encoding='utf-8')
print(json.dumps(manifest))
