import json
from pathlib import Path
import sys

from pypdf import PdfReader

folder = Path(sys.argv[1])
whole = PdfReader(folder / 'printed.pdf')
selected = PdfReader(folder / 'window.print.pdf')
assert len(whole.pages) == 3, 'Должны напечататься все три страницы'
assert len(selected.pages) == 2, 'Должен напечататься диапазон из двух страниц'

def count_structure(image):
  sample = image.convert('RGB').resize((400, 400))
  pixels = (sample.getpixel((x, y)) for y in range(400) for x in range(400))
  return sum(1 for r, g, b in pixels if 205 <= r <= 225 and 228 <= g <= 245 and 232 <= b <= 250)

full_image = whole.pages[0].images[0].image
hidden_image = selected.pages[0].images[0].image
assert full_image.height > full_image.width, 'Исходная страница должна быть портретной'
assert hidden_image.width > hidden_image.height, 'Выбранная страница должна быть повёрнута'
assert count_structure(full_image) > 20000, 'Конструкция должна присутствовать в полной печати'
assert count_structure(hidden_image) < 500, 'Выключенный слой конструкции не должен печататься'
assert len({p.images[0].data for p in selected.pages}) == 2, 'Страницы диапазона не должны дублироваться'
report = {'all_pages': 3, 'selected_pages': 2, 'hidden_layer': 'passed', 'rotation': 'passed', 'distinct_pages': 'passed'}
(folder / 'print-verification.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
print(json.dumps(report))
