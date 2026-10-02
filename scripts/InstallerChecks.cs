using System;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Text;

namespace AstraInstaller {
  public static class Associations {
    [DllImport("shlwapi.dll", CharSet = CharSet.Unicode)]
    public static extern int AssocQueryString(uint flags, uint kind, string association, string extra, StringBuilder output, ref uint length);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    public static extern uint GetLongPathName(string path, StringBuilder output, uint length);

    public static string ShortcutTarget(string path) {
      object instance = new ShellLink();
      try {
        ((IPersistFile)instance).Load(path, 0);
        StringBuilder output = new StringBuilder(32768);
        ((IShellLinkW)instance).GetPath(output, output.Capacity, IntPtr.Zero, 4);
        return output.ToString();
      } finally {
        Marshal.FinalReleaseComObject(instance);
      }
    }

    [ComImport, Guid("00021401-0000-0000-C000-000000000046")]
    private class ShellLink { }

    // Читаем Unicode через системный интерфейс; WScript.Shell искажает кириллицу в английской Windows.
    [ComImport, Guid("000214F9-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IShellLinkW {
      void GetPath([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int length, IntPtr data, uint flags);
    }
  }
}
