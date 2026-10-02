import concurrent.futures
import hashlib
from pathlib import Path
import sys
import urllib.request

url, output, size, digest = sys.argv[1:]
size = int(size)
output = Path(output)
parts = output.with_suffix('.parts')
parts.mkdir(exist_ok=True)
step = 2 * 1024 * 1024

def fetch(start):
  end = min(start + step, size) - 1
  path = parts / str(start)
  if path.exists() and path.stat().st_size == end - start + 1:
    return path
  request = urllib.request.Request(url, headers={'Range': f'bytes={start}-{end}'})
  for attempt in range(3):
    try:
      with urllib.request.urlopen(request, timeout=180) as response:
        if response.status != 206:
          raise RuntimeError(f'Сервер не поддержал диапазон: {response.status}')
        data = response.read()
      if len(data) != end - start + 1:
        raise RuntimeError('Неполный сегмент')
      path.write_bytes(data)
      print(f'Сегмент {start // step + 1} готов', flush=True)
      return path
    except Exception:
      if attempt == 2:
        raise

with concurrent.futures.ThreadPoolExecutor(max_workers=16) as executor:
  paths = list(executor.map(fetch, range(0, size, step)))
with output.open('wb') as target:
  for path in paths:
    target.write(path.read_bytes())
if hashlib.sha256(output.read_bytes()).hexdigest() != digest:
  raise RuntimeError('Не совпала контрольная сумма')
print(f'Готово: {output}', flush=True)
