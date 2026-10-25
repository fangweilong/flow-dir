$shell = New-Object -ComObject Shell.Application
$bin = $shell.Namespace(10)
$items = $bin.Items()
$found = @()
foreach ($item in $items) {
  if ($item.Name -like 'note2*') {
    $found += ($item.Name + ' | ' + $bin.GetDetailsOf($item, 1) + ' | ' + $item.Path)
  }
}
if ($found.Count -eq 0) { 'NOT_IN_RECYCLE_BIN' } else { $found -join "`n" }
