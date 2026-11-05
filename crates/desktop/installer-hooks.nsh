; Ganchos del instalador NSIS de Tauri (D36, D51).
;
; D36: tras copiar los archivos, el instalador ejecuta la preparación de la IA local y espera;
; termina con éxito solo cuando el modelo responde.
;
; D51: el programa se instala en %LOCALAPPDATA%\Programs\NestAdmin y los datos siguen en
; %LOCALAPPDATA%\NestAdmin. La plantilla de Tauri propone %LOCALAPPDATA%\NestAdmin (o la carpeta
; de la instalación anterior, guardada en el registro); solo esa ruta se sustituye. Si el usuario
; elige otra carpeta en el asistente, se respeta. Al instalar encima de una versión anterior se
; borran de la carpeta antigua únicamente los archivos del programa, nunca los datos, y se
; reapuntan los accesos directos que señalaban al ejecutable antiguo.
;
; Este archivo se incluye al principio de la plantilla, antes de las páginas y antes de que se
; definan PRODUCTNAME y MAINBINARYNAME: fuera de las macros las rutas se escriben literalmente.
; Deben coincidir con productName (tauri.conf.json) y con default_data_dir (crates/app).

!define NESTADMIN_DATA_DIR "$LOCALAPPDATA\NestAdmin"
!define NESTADMIN_PROGRAM_DIR "$LOCALAPPDATA\Programs\NestAdmin"

; Carpeta de la instalación anterior, si era otra y aún tenía el ejecutable.
Var NestAdminOldDir

; En el asistente con interfaz, la página de carpeta ya muestra la ruta nueva.
!define MUI_CUSTOMFUNCTION_GUIINIT NestAdminChooseDir

Function NestAdminChooseDir
  ${If} $INSTDIR == "${NESTADMIN_DATA_DIR}"
    StrCpy $INSTDIR "${NESTADMIN_PROGRAM_DIR}"
  ${EndIf}
FunctionEnd

; Reapunta un acceso directo que señalaba al ejecutable de la carpeta antigua.
!macro NestAdminRetarget shortcut
  ${If} ${FileExists} "${shortcut}"
    !insertmacro IsShortcutTarget "${shortcut}" "$NestAdminOldDir\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro SetShortcutTarget "${shortcut}" "$INSTDIR\${MAINBINARYNAME}.exe"
      DetailPrint "Acceso directo actualizado: ${shortcut}"
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; En modo silencioso no hay interfaz: la carpeta se corrige aquí.
  Call NestAdminChooseDir
  SetOutPath $INSTDIR

  StrCpy $NestAdminOldDir ""
  ReadRegStr $R9 SHCTX "${MANUPRODUCTKEY}" ""
  ${If} $R9 != ""
  ${AndIf} $R9 != $INSTDIR
  ${AndIf} ${FileExists} "$R9\${MAINBINARYNAME}.exe"
    StrCpy $NestAdminOldDir $R9
    ; La copia antigua no puede quedar abierta mientras se retira.
    !insertmacro CheckIfAppIsRunning "$NestAdminOldDir\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ${If} $NestAdminOldDir != ""
    DetailPrint "Retirando el programa de la carpeta anterior ($NestAdminOldDir); los datos se conservan."
    !insertmacro NestAdminRetarget "$DESKTOP\${PRODUCTNAME}.lnk"
    !insertmacro NestAdminRetarget "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    !insertmacro NestAdminRetarget "$APPDATA\Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar\${PRODUCTNAME}.lnk"
    ; Solo archivos que instala este paquete. Nunca nestadmin.db, configuracion.json, correo\,
    ; documentos\ ni correo-oauth.json, que en esa carpeta es un archivo del usuario (D43).
    Delete "$NestAdminOldDir\${MAINBINARYNAME}.exe"
    Delete "$NestAdminOldDir\uninstall.exe"
    Delete "$NestAdminOldDir\pdfium\pdfium.dll"
    Delete "$NestAdminOldDir\pdfium\LICENSE"
    RMDir "$NestAdminOldDir\pdfium"
    Delete "$NestAdminOldDir\tessdata\spa.traineddata"
    Delete "$NestAdminOldDir\tessdata\eng.traineddata"
    RMDir "$NestAdminOldDir\tessdata"
    ${If} ${FileExists} "$NestAdminOldDir\tesseract\tesseract.exe"
      RMDir /r "$NestAdminOldDir\tesseract"
    ${EndIf}
    ; Solo desaparece si ha quedado vacía (nunca se abrió NestAdmin).
    RMDir "$NestAdminOldDir"
  ${EndIf}

  DetailPrint "Preparando la IA local de NestAdmin…"
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --preparar' $0
  ${If} $0 != 0
    DetailPrint "La preparación no terminó (código $0); NestAdmin la ofrecerá al abrirse."
    IfSilent +2
    MessageBox MB_ICONINFORMATION|MB_OK "La preparación de la IA local no terminó. NestAdmin la ofrecerá al abrirse."
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; La casilla de Tauri para borrar los datos de la aplicación solo quita la caché de WebView2
  ; (%LOCALAPPDATA%\<identificador>), nunca la carpeta de datos de NestAdmin (D51).
  DetailPrint "Se ha quitado el programa. Los datos de NestAdmin se conservan en ${NESTADMIN_DATA_DIR}."
  DetailPrint "Ollama y el modelo de IA se conservan: pueden usarlos otras aplicaciones."
  IfSilent +2
  MessageBox MB_ICONINFORMATION|MB_OK "NestAdmin se ha desinstalado.$\r$\n$\r$\nSus datos (tareas, documentos, cuentas de correo y configuración) se conservan en ${NESTADMIN_DATA_DIR}, también si marcó borrar los datos de la aplicación, que solo quita la caché de la ventana. Puede borrar esa carpeta a mano si ya no los necesita.$\r$\n$\r$\nOllama y el modelo de IA local también se conservan; puede quitar Ollama desde Aplicaciones instaladas."
!macroend
