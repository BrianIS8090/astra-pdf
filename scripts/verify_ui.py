import ctypes as c
from ctypes import wintypes as w
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import time
from PIL import Image


u = c.WinDLL('user32', use_last_error=True)
k = c.WinDLL('kernel32', use_last_error=True)
papi = c.WinDLL('psapi', use_last_error=True)
u.SetProcessDpiAwarenessContext.argtypes = [c.c_void_p]
u.SetProcessDpiAwarenessContext(c.c_void_p(-4))
LRESULT = c.c_ssize_t
CALLBACK = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
u.EnumWindows.argtypes = [CALLBACK, w.LPARAM]
u.EnumChildWindows.argtypes = [w.HWND, CALLBACK, w.LPARAM]
u.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
u.GetClassNameW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
u.GetWindowTextW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
u.GetDlgCtrlID.argtypes = [w.HWND]
u.GetDlgItem.argtypes = [w.HWND, c.c_int]
u.GetDlgItem.restype = w.HWND
u.IsWindowVisible.argtypes = [w.HWND]
u.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
u.SendMessageTimeoutW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM, w.UINT, w.UINT, c.POINTER(c.c_size_t)]
u.SendMessageTimeoutW.restype = LRESULT
u.SetWindowTextW.argtypes = [w.HWND, w.LPCWSTR]
u.SetFocus.argtypes = [w.HWND]
u.SetFocus.restype = w.HWND
u.AttachThreadInput.argtypes = [w.DWORD, w.DWORD, w.BOOL]
k.GetCurrentThreadId.restype = w.DWORD
u.MoveWindow.argtypes = [w.HWND, c.c_int, c.c_int, c.c_int, c.c_int, w.BOOL]
u.GetWindowRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
k.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
k.OpenProcess.restype = w.HANDLE
k.CloseHandle.argtypes = [w.HANDLE]
k.WaitForSingleObject.argtypes = [w.HANDLE, w.DWORD]
k.VirtualAllocEx.argtypes = [w.HANDLE, c.c_void_p, c.c_size_t, w.DWORD, w.DWORD]
k.VirtualAllocEx.restype = c.c_void_p
k.VirtualFreeEx.argtypes = [w.HANDLE, c.c_void_p, c.c_size_t, w.DWORD]
k.WriteProcessMemory.argtypes = [w.HANDLE, c.c_void_p, c.c_void_p, c.c_size_t, c.POINTER(c.c_size_t)]
k.GlobalAlloc.argtypes = [w.UINT, c.c_size_t]
k.GlobalAlloc.restype = w.HGLOBAL
k.GlobalLock.argtypes = [w.HGLOBAL]
k.GlobalLock.restype = c.c_void_p
k.GlobalUnlock.argtypes = [w.HGLOBAL]


class ProcessEntry(c.Structure):
  _fields_ = [('size', w.DWORD), ('usage', w.DWORD), ('pid', w.DWORD), ('heap', c.c_size_t), ('module', w.DWORD), ('threads', w.DWORD), ('parent', w.DWORD), ('priority', w.LONG), ('flags', w.DWORD), ('exe', w.WCHAR * 260)]


class Memory(c.Structure):
  _fields_ = [('size', w.DWORD), ('faults', w.DWORD)] + [(name, c.c_size_t) for name in ['peak_ws', 'ws', 'peak_paged', 'paged', 'peak_nonpaged', 'nonpaged', 'pagefile', 'peak_pagefile', 'private']]


k.CreateToolhelp32Snapshot.argtypes = [w.DWORD, w.DWORD]
k.CreateToolhelp32Snapshot.restype = w.HANDLE
k.Process32FirstW.argtypes = [w.HANDLE, c.POINTER(ProcessEntry)]
k.Process32NextW.argtypes = [w.HANDLE, c.POINTER(ProcessEntry)]
papi.GetProcessMemoryInfo.argtypes = [w.HANDLE, c.POINTER(Memory), w.DWORD]


def descendants(parent):
  snapshot = k.CreateToolhelp32Snapshot(2, 0)
  entry = ProcessEntry(size=c.sizeof(ProcessEntry))
  rows = []
  ok = k.Process32FirstW(snapshot, c.byref(entry))
  while ok:
    rows.append((entry.pid, entry.parent))
    ok = k.Process32NextW(snapshot, c.byref(entry))
  k.CloseHandle(snapshot)
  result = {parent}
  for _ in range(4):
    result.update(pid for pid, ppid in rows if ppid in result)
  return result


def working_set(pid):
  result = 0
  for child in descendants(pid):
    handle = k.OpenProcess(0x1010, False, child)
    if handle:
      memory = Memory(size=c.sizeof(Memory))
      if papi.GetProcessMemoryInfo(handle, c.byref(memory), c.sizeof(memory)):
        result += memory.ws
      k.CloseHandle(handle)
  return result


def alive(pid):
  handle = k.OpenProcess(0x100000, False, pid)
  if not handle:
    return False
  result = k.WaitForSingleObject(handle, 0) == 258
  k.CloseHandle(handle)
  return result


def info(hwnd):
  cls, title = c.create_unicode_buffer(256), c.create_unicode_buffer(2048)
  u.GetClassNameW(hwnd, cls, len(cls))
  u.GetWindowTextW(hwnd, title, len(title))
  return {'hwnd': hwnd, 'class': cls.value, 'text': title.value, 'id': u.GetDlgCtrlID(hwnd), 'visible': bool(u.IsWindowVisible(hwnd))}


def windows(pid=None, parent=None):
  result = []
  @CALLBACK
  def callback(hwnd, _):
    owner = w.DWORD()
    u.GetWindowThreadProcessId(hwnd, c.byref(owner))
    if pid is None or owner.value == pid:
      result.append(info(hwnd))
    return True
  if parent:
    u.EnumChildWindows(parent, callback, 0)
  else:
    u.EnumWindows(callback, 0)
  return result


def send(hwnd, message, wp=0, lp=0):
  result = c.c_size_t()
  if not u.SendMessageTimeoutW(hwnd, message, wp, lp, 2, 2000, c.byref(result)):
    raise RuntimeError(f'Окно не отвечает: {message:#x}, {c.get_last_error()}')
  return result.value


def set_text(hwnd, value):
  buffer = c.create_unicode_buffer(value)
  assert send(hwnd, 0x0c, 0, c.addressof(buffer))
  actual = c.create_unicode_buffer(len(value.encode('utf-16-le')) // 2 + 1)
  send(hwnd, 0x0d, len(actual), c.addressof(actual))
  assert actual.value == value


def wait(fn, timeout=35):
  deadline = time.monotonic() + timeout
  while time.monotonic() < deadline:
    value = fn()
    if value:
      return value
    time.sleep(.06)
  raise TimeoutError('Не подтверждено ожидаемое состояние окна')


class Viewer:
  def __init__(self, exe, document, output):
    self.output = output
    self.output.parent.mkdir(parents=True, exist_ok=True)
    assert not self.output.exists(), 'Для проверки требуется новый каталог результатов'
    self.started = time.monotonic()
    startup = subprocess.STARTUPINFO()
    startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    startup.wShowWindow = 4
    self.process = subprocess.Popen([str(exe), '--ui-test', str(document), str(output)], startupinfo=startup)
    self.pid = self.process.pid
    self.peak_ws = 0
    self.hwnd = wait(lambda: next((v['hwnd'] for v in windows(self.pid) if v['class'] == 'AstraPdfWindow'), None))
    self.handle = k.OpenProcess(0x1010 | 0x28, False, self.pid)

  def snapshot(self):
    if self.process.poll() is not None:
      raise RuntimeError(f'Просмотрщик завершился раньше времени: {self.process.returncode}')
    send(self.hwnd, 0x8008)
    self.peak_ws = max(self.peak_ws, working_set(self.pid))
    return json.loads(self.output.read_text('utf-8'))

  def ready(self, condition=lambda s: True):
    def inspect():
      state = self.snapshot()
      return state if not state['rendering'] and state['image_hash'] and condition(state) else None
    return wait(inspect)

  def command(self, identifier):
    if identifier in [10, 11]:
      u.PostMessageW(self.hwnd, 0x111, identifier, 0)
    else:
      send(self.hwnd, 0x111, identifier, 0)

  def dialog(self):
    def find():
      classic = next((v for v in windows(self.pid) if v['class'] == '#32770' and v['visible']), None)
      return classic or next((v for v in windows() if v['class'] == 'ApplicationFrameWindow' and v['text'] == 'Astra PDF — печать' and v['visible']), None)
    return wait(find)

  def dismiss(self, identifier=2):
    dialog = self.dialog()
    children = windows(parent=dialog['hwnd'])
    if dialog['class'] == 'ApplicationFrameWindow':
      assert identifier == 2
      u.PostMessageW(dialog['hwnd'], 0x10, 0, 0)
      wait(lambda: not u.IsWindowVisible(dialog['hwnd']))
      wait(lambda: not self.snapshot()['dialog_open'])
      return [dialog] + children
    button = wait(lambda: next((v for v in windows(parent=dialog['hwnd']) if v['class'] == 'Button' and (v['id'] == identifier or (identifier == 1 and v['text'].replace('&', '').upper() in ['OK', 'ОК']))), None))
    u.PostMessageW(button['hwnd'], 0xf5, 0, 0)
    wait(lambda: not u.IsWindowVisible(dialog['hwnd']))
    wait(lambda: not self.snapshot()['dialog_open'])
    return children

  def open(self, document):
    before = self.snapshot()['generation']
    self.command(10)
    dialog = self.dialog()
    children = windows(parent=dialog['hwnd'])
    edits = [v for v in children if v['class'] == 'Edit' and v['visible']]
    self.output.with_suffix('.dialog.json').write_text(json.dumps(children, ensure_ascii=False, indent=2), encoding='utf-8')
    assert len(edits) == 1, 'Неоднозначное поле имени файла в системном диалоге'
    set_text(edits[0]['hwnd'], str(document))
    button = next(v for v in children if v['id'] == 1 and v['class'] == 'Button')
    u.PostMessageW(button['hwnd'], 0xf5, 0, 0)
    wait(lambda: not u.IsWindowVisible(dialog['hwnd']))
    wait(lambda: self.snapshot()['generation'] > before)

  def drop(self, document):
    before = self.snapshot()['generation']
    data = struct.pack('<IiiII', 20, 0, 0, 0, 1) + (str(document) + '\0\0').encode('utf-16le')
    handle = k.GlobalAlloc(0x42, len(data))
    pointer = k.GlobalLock(handle)
    c.memmove(pointer, data, len(data))
    k.GlobalUnlock(handle)
    u.PostMessageW(self.hwnd, 0x233, handle, 0)
    wait(lambda: self.snapshot()['generation'] > before)

  def layer(self, index, enabled):
    control = u.GetDlgItem(self.hwnd, 21)
    # Сообщение ListView передаёт структуру по адресу в памяти процесса окна.
    item = bytearray(96)
    struct.pack_into('<II', item, 12, (2 if enabled else 1) << 12, 0xf000)
    remote = k.VirtualAllocEx(self.handle, None, len(item), 0x3000, 4)
    assert remote
    data = c.create_string_buffer(bytes(item))
    written = c.c_size_t()
    try:
      assert k.WriteProcessMemory(self.handle, remote, data, len(item), c.byref(written))
      send(control, 0x102b, index, remote)
    finally:
      k.VirtualFreeEx(self.handle, remote, 0, 0x8000)

  def page(self, page):
    edit = u.GetDlgItem(self.hwnd, 19)
    # Нажатие в поле задаёт фокус; Enter проходит общий обработчик клавиш окна.
    send(edit, 0x201, 1, 0x00010001)
    send(edit, 0x202, 0, 0x00010001)
    thread = u.GetWindowThreadProcessId(edit, None)
    assert u.AttachThreadInput(k.GetCurrentThreadId(), thread, True)
    try:
      u.SetFocus(edit)
      set_text(edit, str(page))
      send(self.hwnd, 5)
      time.sleep(.2)
      value = c.create_unicode_buffer(32)
      send(edit, 0x0d, 32, c.addressof(value))
      assert value.value == str(page), 'Перерисовка не должна сбрасывать редактируемый номер'
      u.PostMessageW(edit, 0x100, 13, 0)
      return self.ready(lambda s: s['page'] == page - 1)
    finally:
      u.AttachThreadInput(k.GetCurrentThreadId(), thread, False)

  def close(self):
    if self.process.poll() is None:
      for dialog in windows(self.pid):
        if dialog['class'] == '#32770' and dialog['visible']:
          u.PostMessageW(dialog['hwnd'], 0x10, 0, 0)
      for dialog in windows():
        if dialog['class'] == 'ApplicationFrameWindow' and dialog['text'] == 'Astra PDF — печать' and dialog['visible']:
          u.PostMessageW(dialog['hwnd'], 0x10, 0, 0)
      u.PostMessageW(self.hwnd, 0x10, 0, 0)
      self.process.wait(timeout=15)
    assert self.process.returncode == 0
    k.CloseHandle(self.handle)


def run(exe, corpus, output, demo, extra):
  report = {'checks': [], 'documents': []}
  def passed(name):
    report['checks'].append(name)
    print(name, flush=True)
  viewer = Viewer(exe, demo, output / 'main.json')
  try:
    original = viewer.ready()
    assert original['pages'] == 3 and original['states'] == [1, 1, 1]
    initial_hash = original['image_hash']
    assert original['continuous']
    wait(lambda: viewer.snapshot()['thumbnail_count'] >= 2)
    page_list = u.GetDlgItem(viewer.hwnd, 20)
    row_height = send(page_list, 0x1a1, 0)
    send(page_list, 0x197, 0)
    click = ((row_height + 12) << 16) | 50
    send(page_list, 0x201, 1, click)
    send(page_list, 0x202, 0, click)
    viewer.ready(lambda s: s['page'] == 1)
    viewer.page(1)
    passed('thumbnail_pixels_lazy_loading_and_click_navigation')
    canvas = next(v['hwnd'] for v in windows(parent=viewer.hwnd) if v['class'] == 'AstraPdfCanvas')
    send(canvas, 0x20a, (0xff88 << 16))
    boundary = viewer.ready(lambda s: len(s['visible_pages']) >= 2)
    assert boundary['scroll'][1] > 0 and boundary['visible_pages'][:2] == [0, 1]
    send(viewer.hwnd, 0x8009)
    (output / 'continuous-boundary.png').write_bytes(viewer.output.with_suffix('.png').read_bytes())
    send(canvas, 0x115, 7)
    viewer.ready(lambda s: s['page'] == 2)
    send(canvas, 0x115, 6)
    viewer.ready(lambda s: s['page'] == 0 and s['scroll'][1] == 0)
    passed('continuous_wheel_two_pages_and_document_scrollbar')
    viewer.command(27)
    viewer.ready(lambda s: not s['continuous'])
    send(canvas, 0x20a, (0xff88 << 16))
    assert viewer.snapshot()['page'] == 0
    viewer.page(2)
    viewer.command(27)
    viewer.ready(lambda s: s['continuous'] and s['page'] == 1)
    viewer.page(1)
    passed('single_page_mode_and_return_to_continuous')
    for _ in range(4):
      viewer.command(15)
      assert viewer.snapshot()['preview_available'], 'Масштабирование не должно убирать предыдущее изображение'
    viewer.ready(lambda s: s['scale'] > original['scale'])
    viewer.command(16)
    viewer.ready(lambda s: s['image_hash'] == initial_hash)
    passed('rapid_zoom_keeps_document_preview')
    edit = u.GetDlgItem(viewer.hwnd, 19)
    formatting = w.RECT()
    send(edit, 0xb2, 0, c.addressof(formatting))
    assert formatting.top > 0 and formatting.bottom > formatting.top
    u.GetClassLongPtrW.argtypes = [w.HWND, c.c_int]
    u.GetClassLongPtrW.restype = c.c_void_p
    assert u.GetClassLongPtrW(viewer.hwnd, -14), 'У окна должна быть собственная иконка'
    passed('page_number_vertical_padding_and_window_icon')
    viewer.command(25)
    viewer.layer(0, False)
    hidden = viewer.ready(lambda s: s['states'][0] == 0 and s['checks'] == s['states'] and s['image_hash'] != initial_hash)
    viewer.command(22)
    viewer.ready(lambda s: s['states'] == [1, 1, 1] and s['image_hash'] == initial_hash)
    passed('layers_toggle_reset_pixels')
    viewer.page(3)
    viewer.command(24)
    viewer.ready()
    send(viewer.hwnd, 0x8009)
    row = w.RECT()
    send(page_list, 0x198, 2, c.addressof(row))
    list_rect, window_rect = w.RECT(), w.RECT()
    u.GetWindowRect(page_list, c.byref(list_rect))
    u.GetWindowRect(viewer.hwnd, c.byref(window_rect))
    with Image.open(viewer.output.with_suffix('.png')) as screenshot:
      color = screenshot.convert('RGB').getpixel((list_rect.left - window_rect.left + 8, list_rect.top - window_rect.top + row.top + 8))
      assert color == (227, 239, 244), f'Не обновлена подсветка текущей миниатюры: {color}'
    viewer.command(25)
    viewer.page(1)
    u.PostMessageW(viewer.hwnd, 0x100, 0x22, 0)
    viewer.ready(lambda s: s['page'] == 1)
    u.PostMessageW(viewer.hwnd, 0x100, 0x21, 0)
    viewer.ready(lambda s: s['page'] == 0)
    passed('page_number_and_keyboard')
    viewer.command(18)
    rotated = viewer.ready(lambda s: s['rotation'] == 1 and s['image_size'][0] > s['image_size'][1])
    viewer.command(15)
    larger = viewer.ready(lambda s: s['scale'] > rotated['scale'])
    viewer.command(14)
    viewer.ready(lambda s: s['scale'] < larger['scale'])
    viewer.command(17)
    viewer.ready()
    passed('rotation_and_zoom')
    viewer.command(10)
    viewer.dismiss()
    assert viewer.ready()['generation'] == original['generation']
    passed('native_open_cancel')
    viewer.command(11)
    print_controls = viewer.dismiss()
    (output / 'print-dialog.json').write_text(json.dumps(print_controls, ensure_ascii=False, indent=2), encoding='utf-8')
    assert not viewer.snapshot()['printing']
    passed('native_print_cancel')
    for filename in ['broken.pdf', 'empty.pdf', 'password.pdf']:
      viewer.open(corpus / filename)
      viewer.dismiss(1)
      assert viewer.snapshot()['error']
      viewer.open(demo)
      assert viewer.ready()['image_hash'] == initial_hash
    passed('bad_input_and_reopen')
    viewer.open(corpus / 'locked-radio.pdf')
    locked = viewer.ready()
    assert locked['states'] == [1, 0, 1]
    viewer.layer(0, False)
    viewer.ready(lambda s: s['checks'] == [1, 0, 1])
    viewer.layer(1, True)
    viewer.ready(lambda s: s['states'] == [1, 1, 0] and s['checks'] == s['states'])
    passed('locked_and_radio_layers')
    sources = [corpus / file for file in ['text-1000.pdf', 'scan-8.pdf', 'mixed-sizes.pdf', 'layers-300.pdf', 'deep-nesting.pdf']] + extra
    for document in sources:
      before = hashlib.sha256(document.read_bytes()).hexdigest()
      start = time.monotonic()
      viewer.open(document)
      state = viewer.ready()
      elapsed = time.monotonic() - start
      viewer.page(state['pages'])
      viewer.page(1)
      assert state['thumbnail_bytes'] <= 16 * 1024 * 1024 and state['frame_bytes'] <= 96 * 1024 * 1024
      assert hashlib.sha256(document.read_bytes()).hexdigest() == before
      report['documents'].append({'file': str(document), 'bytes': document.stat().st_size, 'pages': state['pages'], 'layers': state['layers'], 'first_frame_ms': state['first_frame_ms'], 'open_dialog_and_render_ms': round(elapsed * 1000), 'sampled_combined_working_set_bytes': working_set(viewer.pid)})
      passed('corpus_' + document.name)
    viewer.drop(demo)
    viewer.ready(lambda s: s['pages'] == 3 and s['layers'] == 3)
    passed('native_drop_unicode_path')
    for width, height in [(1120, 1040), (1800, 1200)]:
      u.MoveWindow(viewer.hwnd, 20, 20, width, height, True)
      time.sleep(.3)
      viewer.ready()
      send(viewer.hwnd, 0x8009)
      (output / f'window-{width}.png').write_bytes(viewer.output.with_suffix('.png').read_bytes())
    passed('resize_and_screenshots')
    canvas = next(v['hwnd'] for v in windows(parent=viewer.hwnd) if v['class'] == 'AstraPdfCanvas')
    for _ in range(5):
      viewer.command(15)
      viewer.ready()
    send(canvas, 0x115, 3)
    assert viewer.snapshot()['scroll'][1] > 0
    send(canvas, 0x115, 6)
    assert viewer.snapshot()['scroll'][1] == 0
    viewer.command(16)
    viewer.ready()
    passed('vertical_scroll_and_fit')
    viewer.open(corpus / 'text-1000.pdf')
    viewer.ready()
    send(viewer.hwnd, 0x800a)
    wait(lambda: viewer.snapshot()['printing'])
    time.sleep(.3)
    viewer.command(23)
    wait(lambda: not viewer.snapshot()['printing'])
    state = viewer.snapshot()
    assert state['cancelled'] and state['print_result'] is False
    viewer.command(18)
    viewer.ready(lambda s: s['rotation'] == 1)
    passed('native_print_abort_and_view_recovery')
    send(viewer.hwnd, 0x800a)
    wait(lambda: viewer.snapshot()['printing'])
    time.sleep(.3)
    viewer.close()
    passed('close_during_native_print')
    report['peak_sampled_combined_working_set_bytes'] = viewer.peak_ws
  finally:
    if viewer.process.poll() is None:
      viewer.close()
  # Закрытие родителя без штатного выхода должно уничтожить дочерний движок.
  child_test = Viewer(exe, demo, output / 'parent-exit.json')
  child_test.ready()
  children = descendants(child_test.pid) - {child_test.pid}
  assert children
  child_test.process.terminate()
  child_test.process.wait(10)
  wait(lambda: not any(alive(pid) for pid in children), 10)
  k.CloseHandle(child_test.handle)
  passed('engine_dies_with_parent')
  report['success'] = True
  (output / 'ui-verification.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
  print(json.dumps(report, ensure_ascii=False), flush=True)


if __name__ == '__main__':
  run(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve(), Path(sys.argv[3]).resolve(), Path(sys.argv[4]).resolve(), [Path(p).resolve() for p in sys.argv[5:]])
