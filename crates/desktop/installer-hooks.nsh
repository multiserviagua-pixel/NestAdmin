; Ganchos del instalador NSIS (D36). Tras copiar los archivos, el instalador ejecuta la
; preparación de la IA local y espera: termina con éxito solo cuando el modelo responde.

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Preparando la IA local de NestAdmin…"
  ExecWait '"$INSTDIR\nestadmin.exe" --preparar' $0
  ${If} $0 != 0
    DetailPrint "La preparación no terminó (código $0); NestAdmin la ofrecerá al abrirse."
    IfSilent +2
    MessageBox MB_ICONINFORMATION|MB_OK "La preparación de la IA local no terminó. NestAdmin la ofrecerá al abrirse."
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DetailPrint "Ollama, el modelo de IA y sus datos se conservan: pueden usarlos otras aplicaciones."
  IfSilent +2
  MessageBox MB_ICONINFORMATION|MB_OK "NestAdmin se ha desinstalado. Ollama, el modelo de IA local y sus datos de NestAdmin se conservan; puede quitar Ollama desde Aplicaciones instaladas."
!macroend
