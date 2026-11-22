; Devil's Peepshow: not running while its files are replaced (and the old Client Tools, which it
; replaces, closed too).
!macro NSIS_HOOK_PREINSTALL
  nsExec::Exec 'taskkill /F /IM "devils-peepshow.exe"'
  Pop $0
  nsExec::Exec 'taskkill /F /IM "devils-rage-client-tools.exe"'
  Pop $0
  nsExec::Exec 'taskkill /F /IM "Devils Rage Client Tools.exe"'
  Pop $0
  Sleep 500
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec 'taskkill /F /IM "devils-peepshow.exe"'
  Pop $0
!macroend
