from pathlib import Path
from PIL import Image, ImageDraw


def make_icon():
  assets = Path(__file__).resolve().parent.parent / 'assets'
  # Векторная эмблема: лист документа и четырёхлучевая звезда Астры.
  svg = '''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256">
  <rect x="16" y="16" width="224" height="224" rx="52" fill="#145e75"/>
  <path d="M76 48 H150 L184 82 V204 H76 Z" fill="#ffffff"/>
  <path d="M150 48 V82 H184" fill="#a9d7df"/>
  <path d="M130 91 L143 121 L173 134 L143 147 L130 178 L117 147 L87 134 L117 121 Z" fill="#157f96"/>
</svg>'''
  (assets / 'app.svg').write_text(svg, encoding='utf-8')
  scale = 4
  image = Image.new('RGBA', (256 * scale, 256 * scale))
  draw = ImageDraw.Draw(image)
  def polygon(points, color):
    draw.polygon([(x * scale, y * scale) for x, y in points], fill=color)
  draw.rounded_rectangle((16 * scale, 16 * scale, 240 * scale, 240 * scale), radius=52 * scale, fill='#145e75')
  polygon([(76, 48), (150, 48), (184, 82), (184, 204), (76, 204)], '#ffffff')
  polygon([(150, 48), (150, 82), (184, 82)], '#a9d7df')
  polygon([(130, 91), (143, 121), (173, 134), (143, 147), (130, 178), (117, 147), (87, 134), (117, 121)], '#157f96')
  image = image.resize((256, 256), Image.Resampling.LANCZOS)
  image.save(assets / 'app.png')
  image.save(assets / 'app.ico', sizes=[(s, s) for s in [16, 20, 24, 32, 40, 48, 64, 128, 256]])
  with Image.open(assets / 'app.ico') as icon:
    assert icon.ico.sizes() == {(s, s) for s in [16, 20, 24, 32, 40, 48, 64, 128, 256]}
    assert icon.convert('RGBA').getextrema()[3] == (0, 255)
  print(str(assets / 'app.ico'))


if __name__ == '__main__':
  make_icon()
