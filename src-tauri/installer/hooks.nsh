; NSIS install hooks (Tauri 2: bundle.windows.nsis.installerHooks).
;
; Tauri's NSIS template creates only a Start Menu shortcut by default. Add a Desktop
; shortcut on install and remove it on uninstall. ${PRODUCTNAME} / ${MAINBINARYNAME}
; are provided by the generated installer.nsi.

!macro NSIS_HOOK_POSTINSTALL
  CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$DESKTOP\${PRODUCTNAME}.lnk"
!macroend
