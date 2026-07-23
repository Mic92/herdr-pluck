use super::error::ClipboardError;
use std::io::Write;

/// Writes an OSC 52 clipboard sequence. Herdr forwards it from the pane to
/// the attached client terminal.
pub(crate) fn write_osc52(output: &mut impl Write, text: &str) -> Result<(), ClipboardError> {
    let payload = base64_encode(text.as_bytes());
    write!(output, "\x1b]52;c;{payload}\x07")
        .and_then(|()| output.flush())
        .map_err(|err| ClipboardError::WriteFailed {
            tool: "osc52".to_string(),
            message: err.to_string(),
        })
}

fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn encodes_text_as_padded_base64() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b"hi"), "aGk=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
        assert_eq!(base64_encode(b""), "");
    }

    #[test]
    fn writes_osc52_sequence_with_clipboard_selection() {
        let mut output = Vec::new();

        write_osc52(&mut output, "hello").expect("osc52 write succeeds");

        assert_eq!(output, b"\x1b]52;c;aGVsbG8=\x07");
    }
}
