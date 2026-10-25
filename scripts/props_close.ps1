Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class PropsClose {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder text, int count);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder text, int count);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
}
"@
$script:n = 0
$cb = [PropsClose+EnumWindowsProc]{
  param($hwnd, $lparam)
  $cls = New-Object System.Text.StringBuilder 64
  [PropsClose]::GetClassNameW($hwnd, $cls, 64) | Out-Null
  if ($cls.ToString() -eq '#32770') {
    $title = New-Object System.Text.StringBuilder 256
    [PropsClose]::GetWindowTextW($hwnd, $title, 256) | Out-Null
    if ($title.ToString() -like '*属性*' -or $title.ToString() -like '*Properties*') {
      [PropsClose]::PostMessageW($hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
      $script:n++
    }
  }
  return $true
}
[PropsClose]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
'closed=' + $script:n
