Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class PropsProbe {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder text, int count);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder text, int count);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
}
"@
$script:found = @()
$cb = [PropsProbe+EnumWindowsProc]{
  param($hwnd, $lparam)
  $cls = New-Object System.Text.StringBuilder 64
  [PropsProbe]::GetClassNameW($hwnd, $cls, 64) | Out-Null
  if ($cls.ToString() -eq '#32770' -and [PropsProbe]::IsWindowVisible($hwnd)) {
    $title = New-Object System.Text.StringBuilder 256
    [PropsProbe]::GetWindowTextW($hwnd, $title, 256) | Out-Null
    if ($title.Length -gt 0) { $script:found += $title.ToString() }
  }
  return $true
}
[PropsProbe]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
if ($script:found.Count -eq 0) { 'NO_DIALOG' } else { $script:found -join ' | ' }
