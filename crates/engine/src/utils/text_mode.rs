//! Text files as the official builds write them. On Windows, CuraEngine's `std::ofstream` and Cura's
//! Python `open(..., "wt")` translate every `\n` into `\r\n`; on Linux and macOS they write the text
//! as it is. `CURUST_TEXT_MODE=crlf|lf` overrides the platform default (for testing).

use std::borrow::Cow;
use std::io::{self, Write};

/// Whether text written to files gets Windows line endings.
pub fn crlf() -> bool {
    match std::env::var("CURUST_TEXT_MODE").ok().as_deref() {
        Some("crlf") => true,
        Some("lf") => false,
        _ => cfg!(windows),
    }
}

/// `text` with the line endings a text-mode file gets on this platform.
pub fn platform_text(text: &str) -> Cow<'_, str> {
    if crlf() && text.contains('\n') {
        Cow::Owned(text.replace('\n', "\r\n"))
    } else {
        Cow::Borrowed(text)
    }
}

/// A writer that performs the text-mode newline translation.
pub struct TextModeWriter<W: Write> {
    inner: W,
    crlf: bool,
}

impl<W: Write> TextModeWriter<W> {
    pub fn new(inner: W) -> Self {
        TextModeWriter { inner, crlf: crlf() }
    }
}

impl<W: Write> Write for TextModeWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.crlf {
            return self.inner.write(buf);
        }
        let mut start = 0;
        for (i, &b) in buf.iter().enumerate() {
            if b == b'\n' {
                self.inner.write_all(&buf[start..i])?;
                self.inner.write_all(b"\r\n")?;
                start = i + 1;
            }
        }
        self.inner.write_all(&buf[start..])?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
