; NSIS install hooks (Tauri 2: bundle.windows.nsis.installerHooks).
;
; Tauri's NSIS template creates only a Start Menu shortcut by default. Add a Desktop
; shortcut on install and remove it on uninstall. ${PRODUCTNAME} / ${MAINBINARYNAME}
; are provided by the generated installer.nsi.
;
; This file is !include'd near the top of installer.nsi (before the MUI bitmap defines
; and the page macros), so top-level !defines here take effect on those pages.

; Show the branding bitmaps at native size instead of stretching them to the MUI image
; control. Tauri defines MUI_WELCOMEFINISHPAGE_BITMAP / MUI_HEADERIMAGE_BITMAP without
; NOSTRETCH, so NSIS stretches header.bmp/sidebar.bmp to fill the control — and the
; control's pixel size depends on the UI font's dialog-unit metrics (the Korean Malgun
; Gothic font yields a different x/y ratio than the 164x314 sidebar was authored for),
; which squashes the square logo horizontally. NOSTRETCH keeps the 1:1 pixels.
!define MUI_WELCOMEFINISHPAGE_BITMAP_NOSTRETCH
!define MUI_UNWELCOMEFINISHPAGE_BITMAP_NOSTRETCH
!define MUI_HEADERIMAGE_BITMAP_NOSTRETCH
!define MUI_HEADERIMAGE_UNBITMAP_NOSTRETCH
; NSIS is the sole file-association authority. Tauri's generated macros overwrite
; backups on reinstall and restore them unconditionally on uninstall, even after
; another application has claimed the extension.
!define EUD_PROJECT_PROGID "EudAgent.Project"
!define EUD_PROJECT_DESCRIPTION "eud-agent 프로젝트"

!macro EUD_NOTIFY_ASSOCIATION_CHANGE
  ; SHCNE_ASSOCCHANGED (0x08000000) with SHCNF_IDLIST (0) refreshes Explorer's
  ; cached extension/icon association without broadcasting a machine-wide setting.
  System::Call 'shell32.dll::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'
!macroend

!macro EUD_RESTORE_PROJECT_EXTENSION EXT
  ReadRegStr $0 SHCTX "Software\Classes\.${EXT}" ""
  ${If} $0 == "${EUD_PROJECT_PROGID}"
    ReadRegStr $1 SHCTX "Software\Classes\.${EXT}" "${EUD_PROJECT_PROGID}_backup"
    ${If} $1 == ""
    ${OrIf} $1 == "${EUD_PROJECT_PROGID}"
      DeleteRegValue SHCTX "Software\Classes\.${EXT}" ""
    ${Else}
      WriteRegStr SHCTX "Software\Classes\.${EXT}" "" "$1"
    ${EndIf}
  ${EndIf}
  DeleteRegValue SHCTX "Software\Classes\.${EXT}" "${EUD_PROJECT_PROGID}_backup"
  DeleteRegKey /ifempty SHCTX "Software\Classes\.${EXT}"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Migrate the old extension before claiming .eap. Its generated backup value
  ; is restored when present, otherwise only our default is cleared.
  !insertmacro EUD_RESTORE_PROJECT_EXTENSION "eudproj"
  ReadRegStr $0 SHCTX "Software\Classes\.eap" ""
  ${If} $0 != "${EUD_PROJECT_PROGID}"
    WriteRegStr SHCTX "Software\Classes\.eap" "${EUD_PROJECT_PROGID}_backup" "$0"
  ${EndIf}
  WriteRegStr SHCTX "Software\Classes\.eap" "" "${EUD_PROJECT_PROGID}"

  CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  WriteRegStr SHCTX "Software\Classes\${EUD_PROJECT_PROGID}" "" "${EUD_PROJECT_DESCRIPTION}"
  WriteRegStr SHCTX "Software\Classes\${EUD_PROJECT_PROGID}\DefaultIcon" "" '"$INSTDIR\icons\project.ico",0'
  WriteRegStr SHCTX "Software\Classes\${EUD_PROJECT_PROGID}\shell" "" "open"
  WriteRegStr SHCTX "Software\Classes\${EUD_PROJECT_PROGID}\shell\open" "" "eud-agent로 열기"
  WriteRegStr SHCTX "Software\Classes\${EUD_PROJECT_PROGID}\shell\open\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'
  !insertmacro EUD_NOTIFY_ASSOCIATION_CHANGE
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$DESKTOP\${PRODUCTNAME}.lnk"
  ; Run only after uninstall has passed its cancel/running-app checks.
  ReadRegStr $0 SHCTX "Software\Classes\${EUD_PROJECT_PROGID}\shell\open\command" ""
  ${If} $0 == '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'
    !insertmacro EUD_RESTORE_PROJECT_EXTENSION "eap"
    !insertmacro EUD_RESTORE_PROJECT_EXTENSION "eudproj"
    DeleteRegValue SHCTX "Software\Classes\${EUD_PROJECT_PROGID}\shell\open\command" ""
    DeleteRegValue SHCTX "Software\Classes\${EUD_PROJECT_PROGID}\shell\open" ""
    DeleteRegValue SHCTX "Software\Classes\${EUD_PROJECT_PROGID}\shell" ""
    DeleteRegValue SHCTX "Software\Classes\${EUD_PROJECT_PROGID}\DefaultIcon" ""
    DeleteRegValue SHCTX "Software\Classes\${EUD_PROJECT_PROGID}" ""
    DeleteRegKey /ifempty SHCTX "Software\Classes\${EUD_PROJECT_PROGID}\shell\open\command"
    DeleteRegKey /ifempty SHCTX "Software\Classes\${EUD_PROJECT_PROGID}\shell\open"
    DeleteRegKey /ifempty SHCTX "Software\Classes\${EUD_PROJECT_PROGID}\shell"
    DeleteRegKey /ifempty SHCTX "Software\Classes\${EUD_PROJECT_PROGID}\DefaultIcon"
    DeleteRegKey /ifempty SHCTX "Software\Classes\${EUD_PROJECT_PROGID}"
  ${EndIf}
  !insertmacro EUD_NOTIFY_ASSOCIATION_CHANGE
!macroend
