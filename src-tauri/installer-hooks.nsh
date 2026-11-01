; Uninstall cleanup for the per-user registry entries FlowDir writes when the user enables
; Explorer integration (see src-tauri/src/shell_integration.rs). Without this the uninstaller
; left context-menu entries, the folder ProgID and the autostart value pointing at a binary
; that no longer exists. The names must stay in sync with `namespace()` ("FlowDir") and the
; "{namespace}.Folder" ProgID.
!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegKey HKCU "Software\Classes\*\shell\FlowDir"
  DeleteRegKey HKCU "Software\Classes\Directory\shell\FlowDir"
  DeleteRegKey HKCU "Software\Classes\FlowDir.Folder"
  DeleteRegValue HKCU "Software\Classes\Directory\OpenWithProgids" "FlowDir.Folder"
  DeleteRegValue HKCU "Software\Classes\Drive\OpenWithProgids" "FlowDir.Folder"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "FlowDir"
!macroend
