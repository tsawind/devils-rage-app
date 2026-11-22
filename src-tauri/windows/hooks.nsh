; 22 Nov: Devil's Peepshow (previews, client bars, spots, hotkeys) is its own program now. The
; app's installer carries Peepshow's installer and runs it quietly, so one install (or update)
; of the app brings both. The old Client Tools it replaces is closed and removed.
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
!macroend

; Devil's Rage Repair (next to the app): a Start menu shortcut, for when the app won't open.
!macro NSIS_HOOK_POSTINSTALL
  CreateShortCut "$SMPROGRAMS\Devils Rage Repair.lnk" "$INSTDIR\Devils Rage Repair.exe"
  ; The old Client Tools (dev.227-249): Devil's Peepshow replaces it.
  Delete "$SMPROGRAMS\Devils Rage Client Tools.lnk"
  Delete "$INSTDIR\devils-rage-client-tools.exe"
  Delete "$INSTDIR\Devils Rage Client Tools.exe"
  ; Devil's Peepshow, next to the app (its own installer: it closes a running Peepshow first and
  ; makes its own Start menu shortcut). /D must come last and unquoted.
  IfFileExists "$INSTDIR\Devils Peepshow setup.exe" 0 peepshow_done
    ExecWait '"$INSTDIR\Devils Peepshow setup.exe" /S /D=$INSTDIR\Peepshow' $0
    DetailPrint "Devil's Peepshow installer finished ($0)"
  peepshow_done:
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$SMPROGRAMS\Devils Rage Repair.lnk"
  Delete "$SMPROGRAMS\Devils Rage Client Tools.lnk"
!macroend
