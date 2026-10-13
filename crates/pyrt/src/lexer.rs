//! Python 3 tokenizer: names, numbers, strings (incl. f-strings), operators and the
//! INDENT / DEDENT / NEWLINE structure.

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Name(String),
    Int(i64),
    BigInt(String),
    Float(f64),
    Imag(f64),
    /// A string literal (prefix-processed), `bytes` flag.
    Str(String, bool),
    /// An f-string: raw source of the literal body (escape-processed per part later).
    FStr(Vec<FPart>),
    Op(&'static str),
    Newline,
    Indent,
    Dedent,
    Eof,
}

/// Part of an f-string.
#[derive(Clone, Debug, PartialEq)]
pub enum FPart {
    Lit(String),
    /// Expression source, conversion (`r`, `s`, `a`), format spec parts, and the `=` debug text.
    Expr { src: String, conv: Option<char>, spec: Vec<FPart>, debug: Option<String> },
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub line: usize,
}

const OPS: &[&str] = &[
    "**=", "//=", ">>=", "<<=", "...", "->", ":=", "**", "//", ">>", "<<", "<=", ">=", "==", "!=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "@=", "+", "-", "*", "/", "%", "@", "&", "|", "^", "~", "<", ">", "(", ")", "[", "]",
    "{", "}", ",", ":", ".", ";", "=",
];

pub fn tokenize(src: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = src.replace("\r\n", "\n").replace('\r', "\n").chars().collect();
    let mut toks: Vec<Token> = Vec::new();
    let mut indents: Vec<usize> = vec![0];
    let mut i = 0;
    let mut line = 1;
    let mut depth = 0i32; // bracket nesting
    let mut at_line_start = true;
    let n = chars.len();
    while i < n {
        if at_line_start && depth == 0 {
            // Measure indentation.
            let mut col = 0;
            let mut j = i;
            while j < n && (chars[j] == ' ' || chars[j] == '\t' || chars[j] == '\x0c') {
                col = if chars[j] == '\t' { (col / 8 + 1) * 8 } else { col + 1 };
                j += 1;
            }
            // Blank or comment-only lines don't count.
            if j >= n {
                break;
            }
            if chars[j] == '\n' {
                i = j + 1;
                line += 1;
                continue;
            }
            if chars[j] == '#' {
                while j < n && chars[j] != '\n' {
                    j += 1;
                }
                i = j;
                continue;
            }
            if chars[j] == '\\' && j + 1 < n && chars[j + 1] == '\n' {
                i = j + 2;
                line += 1;
                continue;
            }
            let cur = *indents.last().unwrap();
            if col > cur {
                indents.push(col);
                toks.push(Token { tok: Tok::Indent, line });
            } else {
                while col < *indents.last().unwrap() {
                    indents.pop();
                    toks.push(Token { tok: Tok::Dedent, line });
                }
                if col != *indents.last().unwrap() {
                    return Err(format!("line {}: unindent does not match any outer indentation level", line));
                }
            }
            i = j;
            at_line_start = false;
        }
        let c = chars[i];
        if c == '\n' {
            if depth == 0 {
                if !matches!(toks.last().map(|t| &t.tok), Some(Tok::Newline) | None) {
                    toks.push(Token { tok: Tok::Newline, line });
                }
                at_line_start = true;
            }
            line += 1;
            i += 1;
            continue;
        }
        if c == ' ' || c == '\t' || c == '\x0c' {
            i += 1;
            continue;
        }
        if c == '#' {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '\\' && i + 1 < n && chars[i + 1] == '\n' {
            i += 2;
            line += 1;
            continue;
        }
        // Strings (with prefixes).
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let lw = word.to_ascii_lowercase();
            if i < n && (chars[i] == '\'' || chars[i] == '"') && matches!(lw.as_str(), "r" | "b" | "f" | "u" | "rb" | "br" | "fr" | "rf") {
                let (tok, ni, nl) = lex_string(&chars, i, &lw, line)?;
                toks.push(Token { tok, line });
                line = nl;
                i = ni;
                continue;
            }
            toks.push(Token { tok: Tok::Name(word), line });
            continue;
        }
        if c == '\'' || c == '"' {
            let (tok, ni, nl) = lex_string(&chars, i, "", line)?;
            toks.push(Token { tok, line });
            line = nl;
            i = ni;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && i + 1 < n && chars[i + 1].is_ascii_digit()) {
            let (tok, ni) = lex_number(&chars, i)?;
            toks.push(Token { tok, line });
            i = ni;
            continue;
        }
        let rest: String = chars[i..(i + 3).min(n)].iter().collect();
        let mut matched = None;
        for op in OPS {
            if rest.starts_with(op) {
                matched = Some(*op);
                break;
            }
        }
        let Some(op) = matched else { return Err(format!("line {}: invalid character {:?}", line, c)) };
        match op {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => depth -= 1,
            _ => {}
        }
        toks.push(Token { tok: Tok::Op(op), line });
        i += op.len();
    }
    if !matches!(toks.last().map(|t| &t.tok), Some(Tok::Newline) | None) {
        toks.push(Token { tok: Tok::Newline, line });
    }
    while indents.len() > 1 {
        indents.pop();
        toks.push(Token { tok: Tok::Dedent, line });
    }
    toks.push(Token { tok: Tok::Eof, line });
    Ok(toks)
}

fn lex_number(c: &[char], mut i: usize) -> Result<(Tok, usize), String> {
    let start = i;
    let n = c.len();
    if c[i] == '0' && i + 1 < n && matches!(c[i + 1], 'x' | 'X' | 'o' | 'O' | 'b' | 'B') {
        let radix = match c[i + 1] {
            'x' | 'X' => 16,
            'o' | 'O' => 8,
            _ => 2,
        };
        i += 2;
        let s = i;
        while i < n && (c[i].is_ascii_alphanumeric() || c[i] == '_') {
            i += 1;
        }
        let digits: String = c[s..i].iter().filter(|ch| **ch != '_').collect();
        return match i64::from_str_radix(&digits, radix) {
            Ok(v) => Ok((Tok::Int(v), i)),
            Err(_) => Err("invalid number literal".into()),
        };
    }
    let mut is_float = false;
    while i < n && (c[i].is_ascii_digit() || c[i] == '_') {
        i += 1;
    }
    if i < n && c[i] == '.' {
        is_float = true;
        i += 1;
        while i < n && (c[i].is_ascii_digit() || c[i] == '_') {
            i += 1;
        }
    }
    if i < n && (c[i] == 'e' || c[i] == 'E') {
        let mut j = i + 1;
        if j < n && (c[j] == '+' || c[j] == '-') {
            j += 1;
        }
        if j < n && c[j].is_ascii_digit() {
            is_float = true;
            i = j;
            while i < n && (c[i].is_ascii_digit() || c[i] == '_') {
                i += 1;
            }
        }
    }
    let text: String = c[start..i].iter().filter(|ch| **ch != '_').collect();
    if i < n && (c[i] == 'j' || c[i] == 'J') {
        return Ok((Tok::Imag(text.parse().map_err(|_| "bad number")?), i + 1));
    }
    if is_float {
        Ok((Tok::Float(text.parse().map_err(|_| "bad float literal")?), i))
    } else {
        match text.parse::<i64>() {
            Ok(v) => Ok((Tok::Int(v), i)),
            Err(_) => Ok((Tok::BigInt(text), i)),
        }
    }
}

/// Process backslash escapes of a (non-raw) string literal body.
pub fn unescape(body: &str, bytes: bool) -> String {
    let c: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] != '\\' || i + 1 >= c.len() {
            out.push(c[i]);
            i += 1;
            continue;
        }
        let e = c[i + 1];
        i += 2;
        match e {
            '\n' => {}
            '\\' => out.push('\\'),
            '\'' => out.push('\''),
            '"' => out.push('"'),
            'a' => out.push('\x07'),
            'b' => out.push('\x08'),
            'f' => out.push('\x0c'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'v' => out.push('\x0b'),
            '0'..='7' => {
                let mut v = e.to_digit(8).unwrap();
                let mut k = 0;
                while k < 2 && i < c.len() && c[i].is_digit(8) {
                    v = v * 8 + c[i].to_digit(8).unwrap();
                    i += 1;
                    k += 1;
                }
                out.push(char::from_u32(v).unwrap_or('?'));
            }
            'x' => {
                let h: String = c[i..(i + 2).min(c.len())].iter().collect();
                i += 2;
                out.push(char::from_u32(u32::from_str_radix(&h, 16).unwrap_or(63)).unwrap_or('?'));
            }
            'u' | 'U' if !bytes => {
                let len = if e == 'u' { 4 } else { 8 };
                let h: String = c[i..(i + len).min(c.len())].iter().collect();
                i += len;
                out.push(char::from_u32(u32::from_str_radix(&h, 16).unwrap_or(63)).unwrap_or('?'));
            }
            'N' if !bytes => {
                // \N{NAME}: only a few common names.
                if i < c.len() && c[i] == '{' {
                    let end = c[i..].iter().position(|&x| x == '}').map(|p| i + p).unwrap_or(c.len());
                    let name: String = c[i + 1..end].iter().collect();
                    i = end + 1;
                    out.push(match name.as_str() {
                        "DEGREE SIGN" => '°',
                        "BULLET" => '•',
                        _ => '?',
                    });
                }
            }
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

fn lex_string(c: &[char], mut i: usize, prefix: &str, mut line: usize) -> Result<(Tok, usize, usize), String> {
    let n = c.len();
    let q = c[i];
    let triple = i + 2 < n && c[i + 1] == q && c[i + 2] == q;
    let qlen = if triple { 3 } else { 1 };
    i += qlen;
    let raw = prefix.contains('r');
    let is_f = prefix.contains('f');
    let bytes = prefix.contains('b');
    let start = i;
    // Find the end (respecting backslashes; for f-strings, nested braces may contain quotes).
    let mut brace = 0i32;
    loop {
        if i >= n {
            return Err(format!("line {}: unterminated string literal", line));
        }
        let ch = c[i];
        if ch == '\\' && i + 1 < n {
            if c[i + 1] == '\n' {
                line += 1;
            }
            i += 2;
            continue;
        }
        if is_f {
            if ch == '{' {
                if i + 1 < n && c[i + 1] == '{' && brace == 0 {
                    i += 2;
                    continue;
                }
                brace += 1;
            } else if ch == '}' {
                if brace > 0 {
                    brace -= 1;
                } else if i + 1 < n && c[i + 1] == '}' {
                    i += 2;
                    continue;
                }
            }
            if brace > 0 && (ch == '\'' || ch == '"') && ch != q {
                // A nested string inside a replacement field.
                let nq = ch;
                i += 1;
                while i < n && c[i] != nq {
                    i += 1;
                }
                i += 1;
                continue;
            }
        }
        if ch == '\n' {
            if !triple {
                return Err(format!("line {}: unterminated string literal", line));
            }
            line += 1;
        }
        if ch == q && brace == 0 {
            if !triple {
                break;
            }
            if i + 2 < n && c[i + 1] == q && c[i + 2] == q {
                break;
            }
        }
        if ch == q && brace > 0 && !triple {
            // Python 3.12: the same quote may appear inside a replacement field.
            let nq = ch;
            i += 1;
            while i < n && c[i] != nq {
                i += 1;
            }
            i += 1;
            continue;
        }
        i += 1;
    }
    let body: String = c[start..i].iter().collect();
    i += qlen;
    if is_f {
        let parts = parse_fstring(&body, raw)?;
        return Ok((Tok::FStr(parts), i, line));
    }
    let text = if raw { body } else { unescape(&body, bytes) };
    Ok((Tok::Str(text, bytes), i, line))
}

/// Split an f-string body into literal parts and replacement fields.
pub fn parse_fstring(body: &str, raw: bool) -> Result<Vec<FPart>, String> {
    let c: Vec<char> = body.chars().collect();
    let mut parts = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    let flush = |lit: &mut String, parts: &mut Vec<FPart>| {
        if !lit.is_empty() {
            let t = if raw { lit.clone() } else { unescape(lit, false) };
            parts.push(FPart::Lit(t));
            lit.clear();
        }
    };
    while i < c.len() {
        let ch = c[i];
        if ch == '{' {
            if i + 1 < c.len() && c[i + 1] == '{' {
                lit.push('{');
                i += 2;
                continue;
            }
            flush(&mut lit, &mut parts);
            // Find the matching close, tracking nesting and strings.
            let mut depth = 0;
            let mut j = i + 1;
            let mut expr_end = None;
            let mut conv = None;
            let mut spec_start = None;
            let mut in_str: Option<char> = None;
            while j < c.len() {
                let d = c[j];
                if let Some(q) = in_str {
                    if d == q {
                        in_str = None;
                    }
                    j += 1;
                    continue;
                }
                match d {
                    '\'' | '"' => in_str = Some(d),
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' => depth -= 1,
                    '}' if depth > 0 => depth -= 1,
                    '}' if depth == 0 => {
                        if expr_end.is_none() {
                            expr_end = Some(j);
                        }
                        break;
                    }
                    '!' if depth == 0 && expr_end.is_none() && j + 1 < c.len() && c[j + 1] != '=' => {
                        expr_end = Some(j);
                        conv = Some(c[j + 1]);
                        j += 1;
                    }
                    ':' if depth == 0 && spec_start.is_none() => {
                        if expr_end.is_none() {
                            expr_end = Some(j);
                        }
                        spec_start = Some(j + 1);
                        // The spec runs to the matching '}' (may contain nested fields).
                        let mut k = j + 1;
                        let mut nest = 0;
                        while k < c.len() {
                            if c[k] == '{' {
                                nest += 1;
                            } else if c[k] == '}' {
                                if nest == 0 {
                                    break;
                                }
                                nest -= 1;
                            }
                            k += 1;
                        }
                        j = k;
                        continue;
                    }
                    _ => {}
                }
                j += 1;
            }
            if j >= c.len() {
                return Err("f-string: expecting '}'".into());
            }
            let ee = expr_end.unwrap_or(j);
            let mut src: String = c[i + 1..ee].iter().collect();
            let mut debug = None;
            let trimmed = src.trim_end();
            if trimmed.ends_with('=') && !trimmed.ends_with("==") && !trimmed.ends_with("!=") && !trimmed.ends_with("<=") && !trimmed.ends_with(">=") {
                debug = Some(src.clone());
                src = trimmed[..trimmed.len() - 1].to_string();
            }
            let spec = match spec_start {
                Some(s) => {
                    let spec_src: String = c[s..j].iter().collect();
                    parse_fstring(&spec_src, raw)?
                }
                None => Vec::new(),
            };
            parts.push(FPart::Expr { src, conv, spec, debug });
            i = j + 1;
            continue;
        }
        if ch == '}' && i + 1 < c.len() && c[i + 1] == '}' {
            lit.push('}');
            i += 2;
            continue;
        }
        lit.push(ch);
        i += 1;
    }
    flush(&mut lit, &mut parts);
    Ok(parts)
}
