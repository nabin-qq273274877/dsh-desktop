; NSIS installer hooks for DeepSeek Harness Desktop.
;
; NSIS_HOOK_PREINSTALL runs BEFORE the installer copies files. We use it to
; terminate any still-running instance of the app (and its child DSH node
; processes), so that dsh-desktop.exe / node.exe are never locked during
; install/update.

!macro NSIS_HOOK_PREINSTALL
  ; Kill the launcher and its DSH child process tree before overwriting files.
  ; nsExec hides any console window from taskkill.
  nsExec::ExecToStack 'taskkill /F /IM "dsh-desktop.exe" /T'
  Pop $0
  nsExec::ExecToStack 'taskkill /F /IM "node.exe" /FI "IMAGENAME eq node.exe" /T'
  Pop $0

  ; Wait for the killed processes to actually release their file handles.
  ; Without this, NSIS can still hit "Error opening file for writing:
  ; ...dsh-desktop.exe" because the freshly-killed process hasn't closed its
  ; handle yet. Poll tasklist until no dsh-desktop.exe remains (max ~10s).
  StrCpy $1 0
preinstall_wait_loop:
  nsExec::ExecToStack 'tasklist /FI "IMAGENAME eq dsh-desktop.exe"'
  Pop $2
  ; tasklist returns 0 when at least one matching process exists (still
  ; running); a non-zero exit means none are left.
  IntCmp $2 0 preinstall_still_running preinstall_done preinstall_done
preinstall_still_running:
  IntOp $1 $1 + 1
  IntCmp $1 20 preinstall_done preinstall_done preinstall_done
  Sleep 500
  Goto preinstall_wait_loop
preinstall_done:
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
