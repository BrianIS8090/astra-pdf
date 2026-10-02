import ctypes as c
import json
from pathlib import Path
import sys
from PIL import Image
from verify_ui import Viewer, send, windows, wait, u, w


def verify(exe, fixture, output):
  viewer = Viewer(exe, fixture / 'source.pdf', output / 'window.json')
  checks = []
  try:
    viewer.ready()
    viewer.command(16)
    state = viewer.ready()
    canvas = next(v['hwnd'] for v in windows(parent=viewer.hwnd) if v['class'] == 'AstraPdfCanvas')

    def drag(bounds):
      state = viewer.snapshot()
      _, x, y = state['page_origins'][0]
      _, _, width, height = state['page_boxes'][0]
      a = (int(x + bounds[0] * width), int(y + bounds[1] * height))
      b = (int(x + bounds[2] * width), int(y + bounds[3] * height))
      send(canvas, 0x201, 1, a[0] | a[1] << 16)
      send(canvas, 0x200, 1, b[0] | b[1] << 16)
      send(canvas, 0x202, 0, b[0] | b[1] << 16)

    def ready():
      return viewer.ready(lambda s: s['editor']['ready'] == len(s['editor']['grids']) and s['editor']['ready'] > 0)

    viewer.command(44)
    ready()
    drag([.02, .02, .98, .98])
    state = ready()
    assert len(state['editor']['masks']) == 1
    checks.append('mouse_selection_creates_content_pixelation')
    send(viewer.hwnd, 0x8009)
    image = Image.open(viewer.output.with_suffix('.png')).convert('RGB')
    image.save(output / 'pixelation.png')
    grid = json.loads((fixture / 'pixel-grid.json').read_text())
    rc, rw = w.RECT(), w.RECT()
    u.GetWindowRect(canvas, c.byref(rc))
    u.GetWindowRect(viewer.hwnd, c.byref(rw))
    _, ox, oy = state['page_origins'][0]
    _, _, pw, ph = state['page_boxes'][0]
    colors = set()
    for gy in range(3, grid['height'] - 3):
      for gx in range(3, grid['width'] - 3):
        x = rc.left - rw.left + ox + int((gx + .5) * pw / grid['width'])
        y = rc.top - rw.top + oy + int((gy + .5) * ph / grid['height'])
        i = (gy * grid['width'] + gx) * 4
        expected = tuple(reversed(grid['bgra'][i:i+3]))
        actual = image.getpixel((x, y))
        assert actual == expected, (gx, gy, actual, expected)
        colors.add(actual)
    assert len(colors) > 10
    checks.append('screen_pixels_match_export_colors')
    masks = state['editor']['masks']
    grid0 = next(g for g in state['editor']['grids'] if g[0] == 0)
    viewer.command(15)
    zoomed = ready()
    assert zoomed['editor']['masks'] == masks and grid0 in zoomed['editor']['grids']
    checks.append('zoom_keeps_physical_pixel_grid')
    viewer.command(18)
    rotated = ready()
    assert rotated['editor']['masks'] == masks and grid0 in rotated['editor']['grids']
    checks.append('rotation_keeps_source_grid')
    for command, mm in [(53, 3), (55, 12), (54, 6)]:
      viewer.command(command)
      current = ready()
      assert current['editor']['masks'][0]['kind']['Pixelate']['block_mm'] == mm
      assert current['editor']['bytes'] <= 16 * 1024 * 1024
    checks.append('three_block_sizes_and_bounded_cache')
    viewer.command(47)
    assert not viewer.snapshot()['editor']['masks']
    checks.append('undo_pixelation')
    for _ in range(3):
      viewer.command(18)
      viewer.ready()
    viewer.command(16)
    viewer.ready()
    viewer.command(52)
    drag([.25, .25, .75, .75])
    assert viewer.snapshot()['editor']['masks'][0]['kind'] == 'Cover'
    checks.append('full_hiding_remains_separate')
    viewer.command(47)
    result = {'passed': True, 'checks': checks}
    (output / 'pixel-ui-verification.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(result))
  finally:
    while viewer.process.poll() is None and viewer.snapshot()['editor']['masks']:
      viewer.command(47)
    viewer.close()


if __name__ == '__main__':
  verify(*(Path(a).resolve() for a in sys.argv[1:]))
