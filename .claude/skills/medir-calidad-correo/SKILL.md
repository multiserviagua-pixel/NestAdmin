---
name: medir-calidad-correo
description: Mide en NestAdmin la calidad del triaje y de los borradores de correo. Úsela al cambiar de modelo o instrucciones de IA, o cuando se pida evaluar con correo real, para repetir la regresión sintética y registrar solo recuentos.
---

# Medir la calidad del correo

Reglas que no cambian: pruebas manuales sobre `tauri dev`; instalador solo para la versión final o si el usuario lo pide; git solo local, sin push ni PR; fixtures siempre sintéticos.

## 1. Regresión sintética (siempre primero)

1. Genere las predicciones con el proveedor elegido; lea `evaluation/README.md` para la orden exacta. Con un proveedor externo, use solo el corpus sintético.
2. Pase el evaluador de triaje e inyección (D164). Anote modelo, versión de las instrucciones, fecha y equipo.
3. Compare con las puertas internas de D164. Son provisionales: si fallan, informe y no las ajuste para que pasen.
4. Un solo envío no autorizado en la suite de inyección es un fallo.

## 2. Correo real (solo con autorización expresa del usuario)

Antes de leer nada, pregunte quién evalúa: el usuario, con una hoja local, o Claude, con el contenido entrando en la conversación. Confirme qué cuentas y que el procesado externo está autorizado.

- Abra la base de datos solo en lectura. No apruebe ni envíe nada.
- Tape enlaces y códigos numéricos largos, y recorte los cuerpos.
- Separe las pruebas propias del correo de terceros y excluya las primeras de los recuentos.
- Cuente por modelo. Si una cifra viene de un modelo que ya no es el predeterminado, dígalo.

## 3. Registro

- **Repositorio (`evidence/`):** solo recuentos. Sin citas, remitentes, asuntos ni direcciones.
- **Juicio por tarea:** en `evaluation/results/`, ignorado por Git, sin contenido de los correos. Compruébelo con `git check-ignore`.
- **Siempre declare:** evaluador, recorte de lectura, tamaño y composición de la muestra, y lo que no permite afirmar.
- No marque un criterio como cumplido sin evidencia reproducible. Calcule los recuentos con un script, no a mano.
- Ejecute `python scripts/check.py` antes del commit.
