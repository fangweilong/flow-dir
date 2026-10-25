param([int]$Key, [int]$Times = 1, [int]$DelayMs = 60)
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class MenuK2 {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder text, int count);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
}
"@
$menu = [IntPtr]::Zero
$cb = [MenuK2+EnumWindowsProc]{
  param($hwnd, $lparam)
  $cls = New-Object System.Text.StringBuilder 64
  [MenuK2]::GetClassNameW($hwnd, $cls, 64) | Out-Null
  if ($cls.ToString() -eq '#32768') { $script:menu = $hwnd }
  return $true
}
[MenuK2]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
if ($menu -eq [IntPtr]::Zero) { 'NO_MENU'; exit }
for ($i = 0; $i -lt $Times; $i++) {
  [MenuK2]::PostMessageW($menu, 0x0100, [IntPtr]$Key, [IntPtr]::Zero) | Out-Null
  [MenuK2]::PostMessageW($menu, 0x0101, [IntPtr]$Key, [IntPtr]::Zero) | Out-Null
  Start-Sleep -Milliseconds $DelayMs
}
'keys sent=' + $Times
