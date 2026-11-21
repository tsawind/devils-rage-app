; Devil's Rage Repair (next to the app): a Start menu shortcut, for when the app won't open.
!macro NSIS_HOOK_POSTINSTALL
  CreateShortCut "$SMPROGRAMS\Devils Rage Repair.lnk" "$INSTDIR\Devils Rage Repair.exe"
  CreateShortCut "$SMPROGRAMS\Devils Rage Client Tools.lnk" "$INSTDIR\Devils Rage Client Tools.exe"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$SMPROGRAMS\Devils Rage Repair.lnk"
  Delete "$SMPROGRAMS\Devils Rage Client Tools.lnk"
!macroend
