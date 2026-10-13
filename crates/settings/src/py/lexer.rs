//! Tokenizer for the Python expression subset used by Cura setting formulas.

use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Name(Rc<str>),
    Int(i64),
    /// An integer literal too large for `i64` (kept as text; evaluated as a float).
    BigInt(String),
    Float(f64),
    Str(Rc<str>),
    /// Keywords and operators.
    Op(&'static str),
    End,
}

const KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except",
    "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try",
    "while", "with", "yield",
];

const OPS: &[&str] = &[
    "**=", "//=", ">>=", "<<=", "...", "**", "//", "<<", ">>", "<=", ">=", "==", "!=", "->", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
    "@=", ":=", "+", "-", "*", "/", "%", "@", "&", "|", "^", "~", "<", ">", "(", ")", "[", "]", "{", "}", ",", ":", ".", ";", "=",
];

fn keyword(s: &str) -> Option<&'static str> {
    KEYWORDS.iter().copied().find(|k| *k == s)
}

pub fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    let mut depth = 0i32;
    while i < b.len() {
        let c = b[i];
        if c == b' ' || c == b'\t' || c == b'\x0c' {
            i += 1;
            continue;
        }
        if c == b'\r' || c == b'\n' {
            if depth == 0 {
                // A line break outside brackets ends the (single) expression; only blank lines may follow.
                let rest = &src[i..];
                if rest.trim().is_empty() {
                    break;
                }
                return Err("invalid syntax (newline)".into());
            }
            i += 1;
            continue;
        }
        if c == b'\\' && i + 1 < b.len() && (b[i + 1] == b'\n' || b[i + 1] == b'\r') {
            i += 2;
            continue;
        }
        if c == b'#' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Names and string prefixes.
        if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 0x80) {
                i += 1;
            }
            let word = &src[start..i];
            if i < b.len() && (b[i] == b'\'' || b[i] == b'"') {
                let lower = word.to_ascii_lowercase();
                if ["r", "u", "b", "br", "rb", "f", "fr", "rf"].contains(&lower.as_str()) {
                    if lower.contains('f') || lower.contains('b') {
                        return Err("unsupported string prefix".into());
                    }
                    let (s, ni) = read_string(src, i, lower.contains('r'))?;
                    i = ni;
                    out.push(Tok::Str(s.into()));
                    continue;
                }
            }
            match keyword(word) {
                Some(k) => out.push(Tok::Op(k)),
                None => out.push(Tok::Name(word.into())),
            }
            continue;
        }
        if c.is_ascii_digit() || (c == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit()) {
            let (t, ni) = read_number(src, i)?;
            i = ni;
            out.push(t);
            continue;
        }
        if c == b'\'' || c == b'"' {
            let (s, ni) = read_string(src, i, false)?;
            i = ni;
            out.push(Tok::Str(s.into()));
            continue;
        }
        let op = OPS.iter().copied().find(|op| src[i..].starts_with(op));
        match op {
            Some(op) => {
                match op {
                    "(" | "[" | "{" => depth += 1,
                    ")" | "]" | "}" => depth -= 1,
                    _ => {}
                }
                out.push(Tok::Op(op));
                i += op.len();
            }
            None => return Err(format!("invalid character {:?}", src[i..].chars().next().unwrap())),
        }
    }
    out.push(Tok::End);
    Ok(out)
}

fn read_number(src: &str, start: usize) -> Result<(Tok, usize), String> {
    let b = src.as_bytes();
    let mut i = start;
    if b[i] == b'0' && i + 1 < b.len() && matches!(b[i + 1], b'x' | b'X' | b'o' | b'O' | b'b' | b'B') {
        let radix = match b[i + 1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            _ => 2,
        };
        i += 2;
        let s = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
            i += 1;
        }
        let digits: String = src[s..i].chars().filter(|c| *c != '_').collect();
        return match i64::from_str_radix(&digits, radix) {
            Ok(v) => Ok((Tok::Int(v), i)),
            Err(_) => Err("invalid number".into()),
        };
    }
    let s = i;
    let mut is_float = false;
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        is_float = true;
        i += 1;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
            i += 1;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            is_float = true;
            i = j;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                i += 1;
            }
        }
    }
    if i < b.len() && (b[i] == b'j' || b[i] == b'J') {
        return Err("complex numbers are not supported".into());
    }
    if i < b.len() && (b[i].is_ascii_alphabetic() || b[i] == b'_') {
        return Err("invalid decimal literal".into());
    }
    let text: String = src[s..i].chars().filter(|c| *c != '_').collect();
    if is_float {
        match text.parse::<f64>() {
            Ok(v) => Ok((Tok::Float(v), i)),
            Err(_) => Err("invalid float".into()),
        }
    } else {
        if text.len() > 1 && text.starts_with('0') && text.bytes().any(|c| c != b'0') {
            return Err("leading zeros in decimal integer literals are not permitted".into());
        }
        match text.parse::<i64>() {
            Ok(v) => Ok((Tok::Int(v), i)),
            Err(_) => Ok((Tok::BigInt(text), i)),
        }
    }
}

fn read_string(src: &str, start: usize, raw: bool) -> Result<(String, usize), String> {
    let b = src.as_bytes();
    let q = b[start];
    let triple = start + 2 < b.len() && b[start + 1] == q && b[start + 2] == q;
    let mut i = start + if triple { 3 } else { 1 };
    let mut out = String::new();
    loop {
        if i >= b.len() {
            return Err("unterminated string literal".into());
        }
        let c = b[i];
        if c == q {
            if !triple {
                return Ok((out, i + 1));
            }
            if i + 2 < b.len() && b[i + 1] == q && b[i + 2] == q {
                return Ok((out, i + 3));
            }
        }
        if !triple && (c == b'\n' || c == b'\r') {
            return Err("unterminated string literal".into());
        }
        if c == b'\\' && i + 1 < b.len() {
            if raw {
                out.push('\\');
                let ch = src[i + 1..].chars().next().unwrap();
                out.push(ch);
                i += 1 + ch.len_utf8();
                continue;
            }
            let e = b[i + 1];
            i += 2;
            match e {
                b'\n' => {}
                b'\\' => out.push('\\'),
                b'\'' => out.push('\''),
                b'"' => out.push('"'),
                b'a' => out.push('\x07'),
                b'b' => out.push('\x08'),
                b'f' => out.push('\x0c'),
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                b'v' => out.push('\x0b'),
                b'0'..=b'7' => {
                    let mut v = (e - b'0') as u32;
                    let mut n = 1;
                    while n < 3 && i < b.len() && (b'0'..=b'7').contains(&b[i]) {
                        v = v * 8 + (b[i] - b'0') as u32;
                        i += 1;
                        n += 1;
                    }
                    out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
                }
                b'x' | b'u' | b'U' => {
                    let n = match e {
                        b'x' => 2,
                        b'u' => 4,
                        _ => 8,
                    };
                    if i + n > b.len() {
                        return Err("truncated escape".into());
                    }
                    let v = u32::from_str_radix(&src[i..i + n], 16).map_err(|_| "truncated escape".to_string())?;
                    out.push(char::from_u32(v).ok_or("invalid escape")?);
                    i += n;
                }
                _ => {
                    out.push('\\');
                    i -= 1;
                    let ch = src[i..].chars().next().unwrap();
                    out.push(ch);
                    i += ch.len_utf8();
                }
            }
            continue;
        }
        let ch = src[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
}
