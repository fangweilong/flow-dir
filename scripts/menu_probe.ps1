Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class MenuProbe {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder text, int count);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
  [DllImport("user32.dll")] public static extern int GetMenuItemCount(IntPtr hMenu);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetMenuStringW(IntPtr hMenu, uint item, StringBuilder text, int count, uint flags);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
}
"@
$menus = @()
$cb = [MenuProbe+EnumWindowsProc]{
  param($hwnd, $lparam)
  $cls = New-Object System.Text.StringBuilder 64
  [MenuProbe]::GetClassNameW($hwnd, $cls, 64) | Out-Null
  if ($cls.ToString() -eq '#32768') {
    $menu = [MenuProbe]::SendMessageW($hwnd, 0x01E1, [IntPtr]::Zero, [IntPtr]::Zero)
    $count = 0
    $items = @()
    if ($menu -ne [IntPtr]::Zero) {
      $count = [MenuProbe]::GetMenuItemCount($menu)
      for ($i = 0; $i -lt $count; $i++) {
        $text = New-Object System.Text.StringBuilder 160
        $len = [MenuProbe]::GetMenuStringW($menu, $i, $text, 160, 0x400)
        if ($len -gt 0) { $items += $text.ToString() } else { $items += '---' }
      }
    }
    $script:menus += [pscustomobject]@{
      hwnd = $hwnd
      visible = [MenuProbe]::IsWindowVisible($hwnd)
      count = $count
      items = ($items -join ' | ')
    }
  }
  return $true
}
[MenuProbe]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
if ($script:menus.Count -eq 0) { 'NO_MENU' } else { $script:menus | Format-List }
