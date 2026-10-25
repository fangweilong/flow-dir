Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class SubProbe {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder text, int count);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
  [DllImport("user32.dll")] public static extern IntPtr GetSubMenu(IntPtr hMenu, int pos);
  [DllImport("user32.dll")] public static extern int GetMenuItemCount(IntPtr hMenu);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetMenuStringW(IntPtr hMenu, uint item, StringBuilder text, int count, uint flags);
}
"@
$hwndMenu = [IntPtr]::Zero
$cb = [SubProbe+EnumWindowsProc]{
  param($hwnd, $lparam)
  $cls = New-Object System.Text.StringBuilder 64
  [SubProbe]::GetClassNameW($hwnd, $cls, 64) | Out-Null
  if ($cls.ToString() -eq '#32768' -and $script:hwndMenu -eq [IntPtr]::Zero) { $script:hwndMenu = $hwnd }
  return $true
}
[SubProbe]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
if ($hwndMenu -eq [IntPtr]::Zero) { 'NO_MENU'; exit }
$menu = [SubProbe]::SendMessageW($hwndMenu, 0x01E1, [IntPtr]::Zero, [IntPtr]::Zero)
$count = [SubProbe]::GetMenuItemCount($menu)
for ($i = 0; $i -lt $count; $i++) {
  $sub = [SubProbe]::GetSubMenu($menu, $i)
  if ($sub -ne [IntPtr]::Zero) {
    $text = New-Object System.Text.StringBuilder 160
    [SubProbe]::GetMenuStringW($menu, $i, $text, 160, 0x400) | Out-Null
    $subCount = [SubProbe]::GetMenuItemCount($sub)
    $subnames = @()
    for ($j = 0; $j -lt [Math]::Min($subCount, 4); $j++) {
      $s = New-Object System.Text.StringBuilder 160
      [SubProbe]::GetMenuStringW($sub, $j, $s, 160, 0x400) | Out-Null
      if ($s.Length -gt 0) { $subnames += $s.ToString() }
    }
    $text.ToString() + ' => ' + $subCount + ' items: ' + ($subnames -join ' / ')
  }
}
