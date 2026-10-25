# Sums the memory of the FlowDir process tree (app + its WebView2 processes).
$app = Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'flowdir.exe' }
if (-not $app) { Write-Output 'flowdir.exe not running'; exit 1 }
$browser = Get-CimInstance Win32_Process | Where-Object {
  $_.Name -eq 'msedgewebview2.exe' -and $_.CommandLine -like '*Local\FlowDir\EBWebView*'
}
$ids = @($app.ProcessId) + @($browser.ProcessId)
$procs = Get-Process -Id $ids -ErrorAction SilentlyContinue
$working = ($procs | Measure-Object -Property WorkingSet64 -Sum).Sum
$private = ($procs | Measure-Object -Property PrivateMemorySize64 -Sum).Sum
Write-Output ("processes={0}" -f $procs.Count)
Write-Output ("working_set_MB={0}" -f [math]::Round($working / 1MB, 1))
Write-Output ("private_MB={0}" -f [math]::Round($private / 1MB, 1))
