//! HTTP/1.1 mínimo para un único `POST` con `Connection: close` (D279). Sin redirecciones, sin
//! cookies, sin proxy. Lee el estado y el cuerpo (`Content-Length`, `chunked` o hasta el cierre)
//! con un límite de tamaño.

use std::io::{self, Read};

/// Tope de la cabecera de la respuesta.
const MAX_HEAD_BYTES: usize = 16 * 1024;

/// Por qué no se pudo leer una respuesta completa.
#[derive(Debug)]
pub(crate) enum ReadFailure {
    /// Error de E/S (o de TLS envuelto en E/S): el llamador lo clasifica.
    Io(io::Error),
    /// El servidor cerró antes de terminar la respuesta.
    Eof,
    TooLarge,
    Invalid,
}

impl From<io::Error> for ReadFailure {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            ReadFailure::Eof
        } else {
            ReadFailure::Io(e)
        }
    }
}

/// Cabecera y cuerpo de una petición `POST`.
pub(crate) fn build_request(host: &str, path: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: text/xml; charset=utf-8\r\n\
         SOAPAction: \"\"\r\nAccept: text/xml\r\nUser-Agent: NestAdmin\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

struct Reader<'a, R: Read> {
    inner: &'a mut R,
    buf: Vec<u8>,
    pos: usize,
}

impl<R: Read> Reader<'_, R> {
    /// Lee más bytes al búfer; `Ok(0)` es el fin limpio de la conexión.
    fn fill(&mut self) -> Result<usize, ReadFailure> {
        // Descarta lo ya consumido para no crecer sin necesidad.
        if self.pos > 0 && self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        }
        let mut chunk = [0u8; 8192];
        loop {
            match self.inner.read(&mut chunk) {
                Ok(n) => {
                    self.buf.extend_from_slice(&chunk[..n]);
                    return Ok(n);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }

    fn available(&self) -> &[u8] {
        &self.buf[self.pos..]
    }

    /// Lee una línea terminada en `\n` (sin el `\r\n`), con tope `limit`.
    fn line(&mut self, limit: usize) -> Result<String, ReadFailure> {
        loop {
            if let Some(i) = self.available().iter().position(|&b| b == b'\n') {
                let raw = &self.available()[..i];
                let text = std::str::from_utf8(raw)
                    .map_err(|_| ReadFailure::Invalid)?
                    .trim_end_matches('\r')
                    .to_string();
                self.pos += i + 1;
                return Ok(text);
            }
            if self.available().len() > limit {
                return Err(ReadFailure::Invalid);
            }
            if self.fill()? == 0 {
                return Err(ReadFailure::Eof);
            }
        }
    }

    /// Exactamente `n` bytes, con el tope ya comprobado por quien llama.
    fn take(&mut self, n: usize) -> Result<Vec<u8>, ReadFailure> {
        while self.available().len() < n {
            if self.fill()? == 0 {
                return Err(ReadFailure::Eof);
            }
        }
        let out = self.available()[..n].to_vec();
        self.pos += n;
        Ok(out)
    }
}

enum Framing {
    Length(usize),
    Chunked,
    UntilClose,
    None,
}

/// Lee una respuesta HTTP/1.x completa. `max_body` limita el cuerpo ya decodificado.
pub(crate) fn read_response<R: Read>(
    inner: &mut R,
    max_body: usize,
) -> Result<(u16, Vec<u8>), ReadFailure> {
    let mut r = Reader {
        inner,
        buf: Vec::new(),
        pos: 0,
    };
    loop {
        // Cabecera: estado y campos.
        let status_line = r.line(MAX_HEAD_BYTES)?;
        let mut parts = status_line.split_whitespace();
        let version = parts.next().ok_or(ReadFailure::Invalid)?;
        if !version.starts_with("HTTP/1.") {
            return Err(ReadFailure::Invalid);
        }
        let status: u16 = parts
            .next()
            .and_then(|s| s.parse().ok())
            .filter(|s| (100..=599).contains(s))
            .ok_or(ReadFailure::Invalid)?;
        let mut length: Option<usize> = None;
        let mut chunked = false;
        let mut head_bytes = status_line.len();
        loop {
            let line = r.line(MAX_HEAD_BYTES)?;
            if line.is_empty() {
                break;
            }
            head_bytes += line.len();
            if head_bytes > MAX_HEAD_BYTES {
                return Err(ReadFailure::Invalid);
            }
            let Some((name, value)) = line.split_once(':') else {
                return Err(ReadFailure::Invalid);
            };
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                let n: usize = value.parse().map_err(|_| ReadFailure::Invalid)?;
                // Dos longitudes distintas son una respuesta ambigua.
                if length.is_some_and(|prev| prev != n) {
                    return Err(ReadFailure::Invalid);
                }
                length = Some(n);
            } else if name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            }
        }
        // Respuestas informativas (100 Continue): se ignoran y se lee la siguiente.
        if (100..200).contains(&status) && status != 101 {
            continue;
        }
        let framing = if status == 204 || status == 304 || status == 101 {
            Framing::None
        } else if chunked {
            Framing::Chunked
        } else if let Some(n) = length {
            Framing::Length(n)
        } else {
            Framing::UntilClose
        };
        let body = match framing {
            Framing::None => Vec::new(),
            Framing::Length(n) => {
                if n > max_body {
                    return Err(ReadFailure::TooLarge);
                }
                r.take(n)?
            }
            Framing::Chunked => read_chunked(&mut r, max_body)?,
            Framing::UntilClose => read_until_close(&mut r, max_body)?,
        };
        return Ok((status, body));
    }
}

fn read_chunked<R: Read>(r: &mut Reader<'_, R>, max_body: usize) -> Result<Vec<u8>, ReadFailure> {
    let mut body = Vec::new();
    loop {
        let line = r.line(MAX_HEAD_BYTES)?;
        let size_text = line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16).map_err(|_| ReadFailure::Invalid)?;
        if size == 0 {
            // Trailers hasta la línea vacía.
            loop {
                if r.line(MAX_HEAD_BYTES)?.is_empty() {
                    return Ok(body);
                }
            }
        }
        if body.len().saturating_add(size) > max_body {
            return Err(ReadFailure::TooLarge);
        }
        body.extend(r.take(size)?);
        if !r.line(MAX_HEAD_BYTES)?.is_empty() {
            return Err(ReadFailure::Invalid);
        }
    }
}

fn read_until_close<R: Read>(
    r: &mut Reader<'_, R>,
    max_body: usize,
) -> Result<Vec<u8>, ReadFailure> {
    loop {
        if r.available().len() > max_body {
            return Err(ReadFailure::TooLarge);
        }
        // Un fin sin `close_notify` llega como error de E/S y se trata como cortado (`Eof`).
        if r.fill()? == 0 {
            return Ok(r.available().to_vec());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &[u8], max: usize) -> Result<(u16, Vec<u8>), ReadFailure> {
        read_response(&mut &raw[..], max)
    }

    #[test]
    fn content_length_body() {
        let (s, b) = parse(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhola!", 100).unwrap();
        assert_eq!((s, b.as_slice()), (200, b"hola!".as_slice()));
    }

    #[test]
    fn chunked_body_with_extension_and_trailer() {
        let raw = b"HTTP/1.1 500 X\r\nTransfer-Encoding: chunked\r\n\r\n4;ext=1\r\nhola\r\n3\r\n bi\r\n0\r\nX-T: 1\r\n\r\n";
        let (s, b) = parse(raw, 100).unwrap();
        assert_eq!((s, b.as_slice()), (500, b"hola bi".as_slice()));
    }

    #[test]
    fn body_until_close_and_truncation() {
        let (s, b) = parse(b"HTTP/1.0 200 OK\r\n\r\ncuerpo", 100).unwrap();
        assert_eq!((s, b.as_slice()), (200, b"cuerpo".as_slice()));
        assert!(matches!(
            parse(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\ncorto", 100),
            Err(ReadFailure::Eof)
        ));
        assert!(matches!(
            parse(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nho",
                100
            ),
            Err(ReadFailure::Eof)
        ));
    }

    #[test]
    fn size_limits_and_malformed_responses() {
        assert!(matches!(
            parse(
                b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n12345678901",
                10
            ),
            Err(ReadFailure::TooLarge)
        ));
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\n123456\r\n6\r\n123456\r\n0\r\n\r\n";
        assert!(matches!(parse(chunked, 10), Err(ReadFailure::TooLarge)));
        assert!(matches!(
            parse(b"basura\r\n\r\n", 10),
            Err(ReadFailure::Invalid)
        ));
        assert!(matches!(
            parse(b"HTTP/1.1 999 X\r\n\r\n", 10),
            Err(ReadFailure::Invalid)
        ));
        assert!(matches!(
            parse(
                b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab",
                10
            ),
            Err(ReadFailure::Invalid)
        ));
    }

    #[test]
    fn informational_responses_are_skipped() {
        let raw = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
        assert_eq!(parse(raw, 10).unwrap().1, b"ok");
    }

    #[test]
    fn request_carries_length_and_close() {
        let req = build_request("h.example", "/p", b"<x/>");
        let text = String::from_utf8(req).unwrap();
        assert!(text.starts_with("POST /p HTTP/1.1\r\nHost: h.example\r\n"));
        assert!(text.contains("Content-Length: 4\r\n"));
        assert!(text.contains("Connection: close\r\n"));
        assert!(text.contains("Content-Type: text/xml; charset=utf-8\r\n"));
        assert!(text.ends_with("\r\n\r\n<x/>"));
    }
}
