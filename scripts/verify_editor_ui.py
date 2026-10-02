import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import sys
import time
from PIL import Image
from verify_ui import Viewer, send, set_text, windows, wait, u, w


u.IsWindowEnabled.argtypes = [w.HWND]


def verify(exe, fixture, output):
  source = fixture / 'source.pdf'
  digest = hashlib.sha256(source.read_bytes()).hexdigest()
  previous_local = os.environ.get('LOCALAPPDATA')
  os.environ['LOCALAPPDATA'] = str(output / 'test-profile')
  try:
    viewer = Viewer(exe, source, output / 'window.json')
  finally:
    if previous_local is None:
      os.environ.pop('LOCALAPPDATA', None)
    else:
      os.environ['LOCALAPPDATA'] = previous_local
  checks = []
  try:
    viewer.ready()
    canvas = next(v['hwnd'] for v in windows(parent=viewer.hwnd) if v['class'] == 'AstraPdfCanvas')

    def state():
      return viewer.snapshot()['editor']

    def control(identifier):
      return next(v['hwnd'] for v in windows(parent=viewer.hwnd) if v['id'] == identifier)

    def button(identifier):
      handle = control(identifier)
      assert u.IsWindowVisible(handle) and u.IsWindowEnabled(handle), identifier
      u.PostMessageW(handle, 0xf5, 0, 0)

    def mode(identifier, expected):
      button(identifier)
      wait(lambda: state()['mode'] == expected)

    def point(x, y):
      s = viewer.snapshot()
      _, ox, oy = s['page_origins'][0]
      _, _, width, height = s['page_boxes'][0]
      return int(ox + x * width) | int(oy + y * height) << 16

    def click(x, y):
      send(canvas, 0x201, 1, point(x, y))
      send(canvas, 0x202, 0, point(x, y))

    def drag(bounds, finish=True):
      send(canvas, 0x201, 1, point(*bounds[:2]))
      send(canvas, 0x200, 1, point(*bounds[2:]))
      if finish:
        send(canvas, 0x202, 0, point(*bounds[2:]))

    def esc():
      u.PostMessageW(viewer.hwnd, 0x100, 27, 0)
      wait(lambda: state()['mode'] == 'View' and not state()['dragging'])

    def capture(name):
      send(viewer.hwnd, 0x8009)
      image = Image.open(viewer.output.with_suffix('.png')).convert('RGB')
      image.save(output / name)
      return image

    mode(44, 'Pixelate')
    image = capture('active-pixel-tool.png')
    rb, rw = w.RECT(), w.RECT()
    u.GetWindowRect(control(44), c.byref(rb))
    u.GetWindowRect(viewer.hwnd, c.byref(rw))
    assert image.getpixel((rb.left - rw.left + 6, rb.top - rw.top + 6)) == (203, 222, 242)
    checks.append('toolbar_icon_active_highlight')
    mode(44, 'View')
    assert not viewer.snapshot()['cancelled']
    checks.append('tool_button_toggles_off')
    mode(44, 'Pixelate')
    button(23)
    wait(lambda: state()['mode'] == 'View')
    assert not viewer.snapshot()['cancelled']
    checks.append('cancel_button_deactivates_idle_tool')
    mode(52, 'Mask')
    drag([.10, .30, .30, .42])
    drag([.60, .55, .82, .70])
    committed = state()['masks']
    assert len(committed) == 2
    button(45)
    confirm = viewer.dialog()
    yes = next(v['hwnd'] for v in windows(parent=confirm['hwnd']) if v['id'] == 6 and v['class'] == 'Button')
    u.PostMessageW(yes, 0xf5, 0, 0)
    save = wait(lambda: next((v for v in windows(viewer.pid) if v['class'] == '#32770' and v['visible'] and v['hwnd'] != confirm['hwnd']), None))
    edits = [v for v in windows(parent=save['hwnd']) if v['class'] == 'Edit' and v['visible']]
    assert len(edits) == 1
    exported = output / 'session-export.pdf'
    set_text(edits[0]['hwnd'], str(exported))
    accept = next(v['hwnd'] for v in windows(parent=save['hwnd']) if v['id'] == 1 and v['class'] == 'Button')
    u.PostMessageW(accept, 0xf5, 0, 0)
    info = wait(lambda: next((v for v in windows(viewer.pid) if v['class'] == 'AstraPdfDialog' and v['visible']), None))
    assert exported.is_file() and state()['masks'] == committed
    u.PostMessageW(info['hwnd'], 0x10, 0, 0)
    wait(lambda: not viewer.snapshot()['dialog_open'])
    checks.append('export_keeps_session_masks_editable')
    drag([.35, .30, .55, .42], finish=False)
    assert state()['dragging']
    esc()
    send(canvas, 0x202, 0, point(.55, .42))
    assert state()['masks'] == committed
    checks.append('escape_discards_only_unfinished_drag')
    click(.2, .35)
    wait(lambda: state()['selected_mask'] == 0)
    assert u.IsWindowVisible(control(57))
    capture('selected-mask.png')
    u.PostMessageW(canvas, 0x100, 46, 0)
    wait(lambda: len(state()['masks']) == 1)
    assert state()['masks'] == [committed[1]]
    checks.append('delete_key_removes_selected_not_last_mask')
    button(47)
    wait(lambda: state()['masks'] == committed)
    checks.append('undo_restores_removed_mask_at_original_position')
    mode(56, 'Masks')
    click(.7, .6)
    wait(lambda: state()['selected_mask'] == 1)
    button(57)
    wait(lambda: len(state()['masks']) == 1)
    assert state()['masks'] == [committed[0]]
    checks.append('floating_mask_action_preserves_other_masks')
    esc()
    while state()['masks']:
      viewer.command(47)
      time.sleep(.04)

    objects = json.loads((fixture / 'objects.json').read_text('utf-8'))
    text_object = next(o for o in objects if o['kind'] == 1 and o['text'])
    bounds = text_object['bounds']
    center = ((bounds[0] + bounds[2]) / 2, (bounds[1] + bounds[3]) / 2)
    mode(41, 'Select')
    click(*center)
    wait(lambda: not state()['busy'] and state()['selected_object'] is not None)
    assert state()['selected_object']['kind'] == 1
    assert all(u.IsWindowVisible(control(i)) and u.IsWindowEnabled(control(i)) for i in [42, 43])
    capture('text-actions.png')
    checks.append('text_selection_shows_floating_actions')
    button(42)
    dialog = wait(lambda: next((v for v in windows(viewer.pid) if v['class'] == 'AstraPdfDialog' and v['visible']), None))
    assert dialog['text'] == 'Изменить текст'
    u.PostMessageW(dialog['hwnd'], 0x100, 27, 0)
    wait(lambda: not viewer.snapshot()['dialog_open'])
    button(43)
    dialog = viewer.dialog()
    viewer.dismiss(7)
    checks.append('floating_actions_open_text_and_delete_dialogs')
    viewer.command(15)
    viewer.ready()
    assert state()['selected_object'] is not None
    for identifier in [42, 43]:
      rc, rb = w.RECT(), w.RECT()
      u.GetWindowRect(canvas, c.byref(rc))
      u.GetWindowRect(control(identifier), c.byref(rb))
      assert rc.left <= rb.left < rb.right <= rc.right
      assert rc.top <= rb.top < rb.bottom <= rc.bottom
    checks.append('floating_actions_follow_zoom_inside_viewport')
    esc()
    assert state()['selected_object'] is None
    assert not u.IsWindowVisible(control(42)) and not u.IsWindowVisible(control(43))
    checks.append('escape_clears_object_selection_and_actions')
    viewer.command(16)
    viewer.ready()
    mode(41, 'Select')
    click(*center)
    viewer.command(23)
    wait(lambda: not state()['busy'])
    assert state()['mode'] == 'View' and state()['selected_object'] is None
    assert not viewer.snapshot()['dialog_open']
    checks.append('cancel_during_object_lookup_does_not_restore_selection')
    mode(41, 'Select')
    click(.5, .5)
    wait(lambda: not state()['busy'] and state()['selected_object'] is not None)
    assert state()['selected_object']['kind'] != 1
    assert not u.IsWindowEnabled(control(42)) and u.IsWindowEnabled(control(43))
    checks.append('nontext_object_disables_text_action')
    esc()
    for width, height in [(1120, 1040), (2200, 1520)]:
      u.MoveWindow(viewer.hwnd, 30, 30, width, height, True)
      viewer.ready()
      rects = []
      for identifier in [41, 44, 52, 56, 47, 45, 23]:
        r = w.RECT()
        u.GetWindowRect(control(identifier), c.byref(r))
        rects.append((r.left, r.top, r.right, r.bottom))
      for i, a in enumerate(rects):
        for b in rects[i+1:]:
          assert a[2] <= b[0] or b[2] <= a[0] or a[3] <= b[1] or b[3] <= a[1]
      capture(f'toolbar-{width}.png')
    checks.append('compact_toolbar_without_overlapping_controls')
    assert hashlib.sha256(source.read_bytes()).hexdigest() == digest
    checks.append('session_actions_leave_source_unchanged')
    result = {'passed': True, 'checks': checks}
    (output / 'editor-ui-verification.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(result))
  finally:
    for dialog in windows(viewer.pid):
      if dialog['visible'] and dialog['class'] in ['AstraPdfDialog', '#32770']:
        u.PostMessageW(dialog['hwnd'], 0x10, 0, 0)
    for _ in range(30):
      if not state()['masks']:
        break
      viewer.command(47)
      time.sleep(.04)
    viewer.close()


if __name__ == '__main__':
  verify(*(Path(a).resolve() for a in sys.argv[1:]))
