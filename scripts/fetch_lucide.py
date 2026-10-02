"""Обновление закреплённых исходников иконок Lucide; в обычной сборке сеть не нужна."""
import json
from pathlib import Path
import urllib.request

ROOT = Path(__file__).resolve().parents[1] / 'assets' / 'lucide'
NAMES = ['folder-open', 'save', 'printer', 'chevron-left', 'chevron-right',
  'minus', 'plus', 'scan', 'move-horizontal', 'rotate-cw', 'scroll',
  'mouse-pointer-2', 'grid-3x3', 'shield', 'scan-line', 'undo-2', 'x',
  'text-cursor-input', 'trash', 'files', 'layers', 'rotate-ccw']

if __name__ == '__main__':
  ROOT.mkdir(parents=True, exist_ok=True)
  req = urllib.request.Request('https://api.github.com/repos/lucide-icons/lucide/commits/main', headers={'User-Agent': 'AstraPDF'})
  sha = json.load(urllib.request.urlopen(req))['sha']
  for name in NAMES + ['LICENSE']:
    remote = 'LICENSE' if name == 'LICENSE' else f'icons/{name}.svg'
    data = urllib.request.urlopen(f'https://raw.githubusercontent.com/lucide-icons/lucide/{sha}/{remote}').read()
    (ROOT / ('LICENSE.txt' if name == 'LICENSE' else name + '.svg')).write_bytes(data)
  (ROOT / 'source.json').write_text(json.dumps({'repository': 'https://github.com/lucide-icons/lucide', 'commit': sha, 'icons': NAMES}, indent=2), 'utf-8')
  print(sha)

