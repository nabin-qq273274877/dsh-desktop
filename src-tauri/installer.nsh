; NSIS installer hooks for DeepSeek Harness Desktop.
;
; NSIS_HOOK_PREINSTALL runs BEFORE the installer copies files. We use it to
; terminate any still-running instance of the app (and its child DSH node
; processes), so that node.exe is never locked during install/update.

!macro NSIS_HOOK_PREINSTALL
  ; Kill the launcher and its DSH child process tree before overwriting files.
  ; nsExec hides any console window from taskkill.
  nsExec::ExecToStack 'taskkill /F /IM "dsh-desktop.exe" /T'
  Pop $0
  nsExec::ExecToStack 'taskkill /F /IM "node.exe" /FI "IMAGENAME eq node.exe" /T'
  Pop $0
!macroend

; Nothing needed after install/uninstall for now.
!macro NSIS_HOOK_POSTINSTALL
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToStack 'taskkill /F /IM "dsh-desktop.exe" /T'
  Pop $0
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
