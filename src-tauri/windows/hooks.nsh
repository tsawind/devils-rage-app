; 21 Nov: Client Tools (and an older copy of it) must not be running while files are replaced.
!macro NSIS_HOOK_PREINSTALL
  nsExec::Exec 'taskkill /F /IM "devils-rage-client-tools.exe"'
  Pop $0
  nsExec::Exec 'taskkill /F /IM "Devils Rage Client Tools.exe"'
  Pop $0
  Sleep 500
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec 'taskkill /F /IM "devils-rage-client-tools.exe"'
  Pop $0
  nsExec::Exec 'taskkill /F /IM "Devils Rage Client Tools.exe"'
  Pop $0
!macroend

; Devil's Rage Repair (next to the app): a Start menu shortcut, for when the app won't open.
!macro NSIS_HOOK_POSTINSTALL
  CreateShortCut "$SMPROGRAMS\Devils Rage Repair.lnk" "$INSTDIR\Devils Rage Repair.exe"
  CreateShortCut "$SMPROGRAMS\Devils Rage Client Tools.lnk" "$INSTDIR\devils-rage-client-tools.exe"
  ; The renamed copy older installs put next to the app (21 Nov: Tauri ships the program itself).
  Delete "$INSTDIR\Devils Rage Client Tools.exe"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$SMPROGRAMS\Devils Rage Repair.lnk"
  Delete "$SMPROGRAMS\Devils Rage Client Tools.lnk"
!macroend
