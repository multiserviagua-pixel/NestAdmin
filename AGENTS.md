# Instrucciones del proyecto

- Lea `PLAN.md` y las especificaciones de `specs/v1/` antes de cambiar el comportamiento. Mantenga requisitos, contratos, matriz y pruebas alineados; registre los cambios de alcance.
- Priorice el procesamiento local. Nunca añada respaldo automático hacia un servicio de IA externo. Las acciones de envío y registro requieren revisión salvo una regla expresa activa.
- No incluya documentos reales, credenciales, datos personales ni resultados privados en fixtures, registros o el repositorio. Use exclusivamente datos sintéticos para las pruebas y mantenga los resultados de evaluación separados del contenido de usuario.
- Evite efectos duplicados al recuperar trabajos y cubra los fallos de modelos, OCR, red y credenciales. La ausencia de datos debe ser explícita.
- Ejecute `python scripts/check.py` al modificar la documentación. Ejecute además las pruebas y comprobaciones pertinentes del código que se implemente. El verificador documental no acredita que exista o funcione la aplicación.
- No marque criterios de aceptación como cumplidos sin evidencia reproducible. Las pruebas de instalador, escritorio y rendimiento requieren Windows y, cuando corresponda, el hardware objetivo.
