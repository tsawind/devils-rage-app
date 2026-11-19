; Devil's Rage Repair (next to the app): a Start menu shortcut, for when the app won't open.
!macro NSIS_HOOK_POSTINSTALL
  CreateShortCut "$SMPROGRAMS\Devils Rage Repair.lnk" "$INSTDIR\Devils Rage Repair.exe"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$SMPROGRAMS\Devils Rage Repair.lnk"
!macroend
