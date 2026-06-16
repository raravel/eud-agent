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

!macro NSIS_HOOK_POSTINSTALL
  CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$DESKTOP\${PRODUCTNAME}.lnk"
!macroend
