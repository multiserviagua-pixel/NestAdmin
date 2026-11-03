# Evidencia T03 (parcial): selección de proveedor sin respaldo

Cubre la parte de R03 que no depende de la interfaz ni de una cuenta de Groq, en `crates/ai`. Los adaptadores hablan con un servidor HTTP simulado en 127.0.0.1; ninguna prueba sale a la red.

## Cómo reproducir

```sh
cargo test -p nestadmin-ai --locked
```

## Ejecución registrada

- Fecha: 2026-11-03
- Plataforma: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB
- Rust 1.99.1 (MSVC), `ureq` 3.5.1
- Resultado: 9 pruebas superadas, 0 fallidas.

| Prueba | Qué comprueba |
| --- | --- |
| `local_provider_is_the_default_choice` | La selección inicial es Ollama en `127.0.0.1:11434` con `qwen3:8b`. |
| `ollama_accepts_only_loopback_addresses` | El adaptador local rechaza HTTPS, hosts remotos, dominios que empiezan por `127.0.0.1.`, URL con usuario y direcciones sin esquema. |
| `ollama_extraction_sends_schema_without_reasoning_and_reports_model` | La petición lleva esquema JSON, temperatura 0, sin streaming ni razonamiento; el resultado informa proveedor, modelo y uso, y un `null` del modelo es dato ausente. |
| `contract_errors_from_the_local_provider` | Puerto cerrado → `unavailable`; HTTP 500 → `unavailable`; texto no JSON o JSON que no es objeto → `invalid_response`; respuesta lenta → `timeout`. |
| `local_failure_never_reaches_the_external_provider` | Con Groq configurado y con credencial pero no seleccionado, tres fallos locales no generan ninguna petición al servidor externo y la selección se mantiene. |
| `external_provider_requires_explicit_choice_and_credential` | Sin credencial, elegir Groq falla con `unauthorized` sin crear cliente; con credencial, se construye solo al elegirlo. |
| `groq_sends_bearer_key_and_json_mode_and_parses_output` | El adaptador externo usa la clave como `Bearer`, modo JSON y el esquema en las instrucciones. |
| `rejected_key_is_unauthorized_and_never_printed` | HTTP 401 → `unauthorized`; ni el error, ni `Debug` del proveedor, ni el secreto muestran la clave. |
| `model_output_outside_the_contract_is_invalid` | Tipos no textuales, categorías fuera de la lista o campos ausentes en la salida son `invalid_response`. |

## Pendiente para completar T03

- Selección explícita desde la interfaz y persistencia de la elección (fase 6).
- Prueba con la API real de Groq: requiere una clave del usuario en el administrador de credenciales.
- Observación del tráfico de red de la aplicación completa en modo local (junto con T09).
