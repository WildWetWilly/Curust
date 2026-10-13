//! A small JSON DOM that mirrors the behaviour of RapidJSON (as used by CuraEngine with its
//! default parse flags): object members keep their order (duplicates allowed, lookups find the
//! first match), numbers are parsed with RapidJSON's "normal precision" algorithm and integer
//! values remember their integer type.

use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    /// Integers that fit in an `int`/`unsigned`/`int64_t`/`uint64_t` (RapidJSON keeps them exact).
    Int(i64),
    Uint(u64),
    Double(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

impl JsonValue {
    pub fn is_object(&self) -> bool {
        matches!(self, JsonValue::Object(_))
    }
    pub fn is_string(&self) -> bool {
        matches!(self, JsonValue::String(_))
    }
    pub fn is_number(&self) -> bool {
        matches!(self, JsonValue::Int(_) | JsonValue::Uint(_) | JsonValue::Double(_))
    }
    pub fn is_array(&self) -> bool {
        matches!(self, JsonValue::Array(_))
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::String(s) => Some(s),
            _ => None,
        }
    }
    /// `GetDouble()`.
    pub fn get_double(&self) -> f64 {
        match self {
            JsonValue::Int(i) => *i as f64,
            JsonValue::Uint(u) => *u as f64,
            JsonValue::Double(d) => *d,
            _ => panic!("not a number"),
        }
    }
    pub fn members(&self) -> &[(String, JsonValue)] {
        match self {
            JsonValue::Object(m) => m,
            _ => &[],
        }
    }
    pub fn elements(&self) -> &[JsonValue] {
        match self {
            JsonValue::Array(a) => a,
            _ => &[],
        }
    }
    /// `HasMember`.
    pub fn has_member(&self, name: &str) -> bool {
        self.get(name).is_some()
    }
    /// `FindMember` / `operator[]`: the first member with the given name.
    pub fn get(&self, name: &str) -> Option<&JsonValue> {
        match self {
            JsonValue::Object(m) => m.iter().find(|(k, _)| k == name).map(|(_, v)| v),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct JsonError {
    pub offset: usize,
    pub message: &'static str,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (offset {})", self.message, self.offset)
    }
}

fn pow10_table() -> &'static [f64; 309] {
    static TABLE: OnceLock<[f64; 309]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [0.0f64; 309];
        for (n, v) in t.iter_mut().enumerate() {
            *v = format!("1e{}", n).parse::<f64>().unwrap();
        }
        t
    })
}

fn fast_path(significand: f64, exp: i32) -> f64 {
    if exp < -308 {
        0.0
    } else if exp >= 0 {
        significand * pow10_table()[exp as usize]
    } else {
        significand / pow10_table()[(-exp) as usize]
    }
}

fn strtod_normal_precision(mut d: f64, p: i32) -> f64 {
    if p < -308 {
        // Prevent expSum < -308, making Pow10(p) = 0
        d = fast_path(d, -308);
        d = fast_path(d, p + 308);
    } else {
        d = fast_path(d, p);
    }
    d
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> u8 {
        if self.pos < self.s.len() {
            self.s[self.pos]
        } else {
            0
        }
    }

    fn take(&mut self) -> u8 {
        let c = self.peek();
        self.pos += 1;
        c
    }

    fn consume(&mut self, c: u8) -> bool {
        if self.peek() == c {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn err<T>(&self, message: &'static str, offset: usize) -> Result<T, JsonError> {
        Err(JsonError { offset, message })
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), b' ' | b'\n' | b'\r' | b'\t') {
            self.pos += 1;
        }
    }

    fn parse_value(&mut self) -> Result<JsonValue, JsonError> {
        match self.peek() {
            b'n' => self.parse_literal(b"null", JsonValue::Null),
            b't' => self.parse_literal(b"true", JsonValue::Bool(true)),
            b'f' => self.parse_literal(b"false", JsonValue::Bool(false)),
            b'"' => Ok(JsonValue::String(self.parse_string()?)),
            b'{' => self.parse_object(),
            b'[' => self.parse_array(),
            _ => self.parse_number(),
        }
    }

    fn parse_literal(&mut self, lit: &[u8], v: JsonValue) -> Result<JsonValue, JsonError> {
        let start = self.pos;
        for &c in lit {
            if !self.consume(c) {
                return self.err("Invalid value.", start);
            }
        }
        Ok(v)
    }

    fn parse_hex4(&mut self) -> Result<u32, JsonError> {
        let mut code = 0u32;
        for _ in 0..4 {
            let c = self.take();
            code <<= 4;
            code += match c {
                b'0'..=b'9' => (c - b'0') as u32,
                b'A'..=b'F' => (c - b'A' + 10) as u32,
                b'a'..=b'f' => (c - b'a' + 10) as u32,
                _ => return self.err("Incorrect hex digit after \\u escape in string.", self.pos - 1),
            };
        }
        Ok(code)
    }

    fn parse_string(&mut self) -> Result<String, JsonError> {
        let start = self.pos;
        self.take(); // '"'
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = self.peek();
            if c == b'\\' {
                let escape_offset = self.pos;
                self.take();
                let e = self.take();
                match e {
                    b'"' => out.push(b'"'),
                    b'\\' => out.push(b'\\'),
                    b'/' => out.push(b'/'),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0C),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'u' => {
                        let mut codepoint = self.parse_hex4()?;
                        if (0xD800..=0xDBFF).contains(&codepoint) {
                            // high surrogate, check following low surrogate
                            if !(self.consume(b'\\') && self.consume(b'u')) {
                                return self.err("The surrogate pair in string is invalid.", escape_offset);
                            }
                            let codepoint2 = self.parse_hex4()?;
                            if !(0xDC00..=0xDFFF).contains(&codepoint2) {
                                return self.err("The surrogate pair in string is invalid.", escape_offset);
                            }
                            codepoint = (((codepoint - 0xD800) << 10) | (codepoint2 - 0xDC00)) + 0x10000;
                        } else if (0xDC00..=0xDFFF).contains(&codepoint) {
                            return self.err("The surrogate pair in string is invalid.", escape_offset);
                        }
                        let ch = char::from_u32(codepoint).unwrap_or('\u{FFFD}');
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                    _ => return self.err("Invalid escape character in string.", escape_offset),
                }
            } else if c == b'"' {
                self.take();
                break;
            } else if self.pos >= self.s.len() {
                return self.err("Missing a closing quotation mark in string.", self.pos);
            } else if c < 0x20 {
                return self.err("Invalid encoding in string.", self.pos);
            } else {
                out.push(self.take());
            }
        }
        let _ = start;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    fn parse_object(&mut self) -> Result<JsonValue, JsonError> {
        self.take(); // '{'
        let mut members = Vec::new();
        self.skip_whitespace();
        if self.consume(b'}') {
            return Ok(JsonValue::Object(members));
        }
        loop {
            if self.peek() != b'"' {
                return self.err("Missing a name for object member.", self.pos);
            }
            let name = self.parse_string()?;
            self.skip_whitespace();
            if !self.consume(b':') {
                return self.err("Missing a colon after a name of object member.", self.pos);
            }
            self.skip_whitespace();
            let value = self.parse_value()?;
            members.push((name, value));
            self.skip_whitespace();
            match self.peek() {
                b',' => {
                    self.take();
                    self.skip_whitespace();
                }
                b'}' => {
                    self.take();
                    return Ok(JsonValue::Object(members));
                }
                _ => return self.err("Missing a comma or '}' after an object member.", self.pos),
            }
        }
    }

    fn parse_array(&mut self) -> Result<JsonValue, JsonError> {
        self.take(); // '['
        let mut elements = Vec::new();
        self.skip_whitespace();
        if self.consume(b']') {
            return Ok(JsonValue::Array(elements));
        }
        loop {
            elements.push(self.parse_value()?);
            self.skip_whitespace();
            if self.consume(b',') {
                self.skip_whitespace();
            } else if self.consume(b']') {
                return Ok(JsonValue::Array(elements));
            } else {
                return self.err("Missing a comma or ']' after an array element.", self.pos);
            }
        }
    }

    fn is_digit(c: u8) -> bool {
        c.is_ascii_digit()
    }

    /// Port of `GenericReader::ParseNumber` with default flags.
    fn parse_number(&mut self) -> Result<JsonValue, JsonError> {
        let start_offset = self.pos;
        let mut d = 0.0f64;
        let minus = self.consume(b'-');

        // Parse int: zero / ( digit1-9 *DIGIT )
        let mut i: u32;
        let mut i64v: u64 = 0;
        let mut use64bit = false;
        let mut significand_digit: i32 = 0;
        if self.peek() == b'0' {
            i = 0;
            self.take();
        } else if (b'1'..=b'9').contains(&self.peek()) {
            i = (self.take() - b'0') as u32;
            if minus {
                while Self::is_digit(self.peek()) {
                    if i >= 214748364 {
                        // 2^31 = 2147483648
                        if i != 214748364 || self.peek() > b'8' {
                            i64v = i as u64;
                            use64bit = true;
                            break;
                        }
                    }
                    i = i * 10 + (self.take() - b'0') as u32;
                    significand_digit += 1;
                }
            } else {
                while Self::is_digit(self.peek()) {
                    if i >= 429496729 {
                        // 2^32 - 1 = 4294967295
                        if i != 429496729 || self.peek() > b'5' {
                            i64v = i as u64;
                            use64bit = true;
                            break;
                        }
                    }
                    i = i * 10 + (self.take() - b'0') as u32;
                    significand_digit += 1;
                }
            }
        } else {
            return self.err("Invalid value.", self.pos);
        }

        // Parse 64bit int
        let mut use_double = false;
        if use64bit {
            if minus {
                while Self::is_digit(self.peek()) {
                    if i64v >= 0x0CCCCCCCCCCCCCCC {
                        // 2^63 = 9223372036854775808
                        if i64v != 0x0CCCCCCCCCCCCCCC || self.peek() > b'8' {
                            d = i64v as f64;
                            use_double = true;
                            break;
                        }
                    }
                    i64v = i64v * 10 + (self.take() - b'0') as u64;
                    significand_digit += 1;
                }
            } else {
                while Self::is_digit(self.peek()) {
                    if i64v >= 0x1999999999999999 {
                        // 2^64 - 1 = 18446744073709551615
                        if i64v != 0x1999999999999999 || self.peek() > b'5' {
                            d = i64v as f64;
                            use_double = true;
                            break;
                        }
                    }
                    i64v = i64v * 10 + (self.take() - b'0') as u64;
                    significand_digit += 1;
                }
            }
        }

        // Force double for big integer
        if use_double {
            while Self::is_digit(self.peek()) {
                d = d * 10.0 + (self.take() - b'0') as f64;
            }
        }

        // Parse frac = decimal-point 1*DIGIT
        let mut exp_frac: i32 = 0;
        if self.consume(b'.') {
            if !Self::is_digit(self.peek()) {
                return self.err("Missing fraction part in number.", self.pos);
            }

            if !use_double {
                // Use i64 to store significand in 64-bit architecture
                if !use64bit {
                    i64v = i as u64;
                }

                while Self::is_digit(self.peek()) {
                    if i64v > 0x1FFFFFFFFFFFFF {
                        // 2^53 - 1 for fast path
                        break;
                    } else {
                        i64v = i64v * 10 + (self.take() - b'0') as u64;
                        exp_frac -= 1;
                        if i64v != 0 {
                            significand_digit += 1;
                        }
                    }
                }

                d = i64v as f64;
                use_double = true;
            }

            while Self::is_digit(self.peek()) {
                if significand_digit < 17 {
                    d = d * 10.0 + (self.take() - b'0') as f64;
                    exp_frac -= 1;
                    if d > 0.0 {
                        significand_digit += 1;
                    }
                } else {
                    self.take();
                }
            }
        }

        // Parse exp = e [ minus / plus ] 1*DIGIT
        let mut exp: i32 = 0;
        if self.consume(b'e') || self.consume(b'E') {
            if !use_double {
                d = if use64bit { i64v as f64 } else { i as f64 };
                use_double = true;
            }

            let mut exp_minus = false;
            if self.consume(b'+') {
            } else if self.consume(b'-') {
                exp_minus = true;
            }

            if Self::is_digit(self.peek()) {
                exp = (self.take() - b'0') as i32;
                if exp_minus {
                    let max_exp = (exp_frac + 2147483639) / 10;
                    while Self::is_digit(self.peek()) {
                        exp = exp * 10 + (self.take() - b'0') as i32;
                        if exp > max_exp {
                            while Self::is_digit(self.peek()) {
                                // Consume the rest of exponent
                                self.take();
                            }
                        }
                    }
                } else {
                    // positive exp
                    let max_exp = 308 - exp_frac;
                    while Self::is_digit(self.peek()) {
                        exp = exp * 10 + (self.take() - b'0') as i32;
                        if exp > max_exp {
                            return self.err("Number too big to be stored in double.", start_offset);
                        }
                    }
                }
            } else {
                return self.err("Missing exponent in number.", self.pos);
            }

            if exp_minus {
                exp = -exp;
            }
        }

        if use_double {
            let p = exp + exp_frac;
            d = strtod_normal_precision(d, p);
            if d > f64::MAX {
                return self.err("Number too big to be stored in double.", start_offset);
            }
            Ok(JsonValue::Double(if minus { -d } else { d }))
        } else if use64bit {
            if minus {
                Ok(JsonValue::Int((!i64v).wrapping_add(1) as i64))
            } else {
                Ok(JsonValue::Uint(i64v))
            }
        } else if minus {
            Ok(JsonValue::Int((!i).wrapping_add(1) as i32 as i64))
        } else {
            Ok(JsonValue::Uint(i as u64))
        }
    }
}

/// Parse a complete JSON document (`Document::ParseStream` with default flags).
pub fn parse(text: &[u8]) -> Result<JsonValue, JsonError> {
    let mut p = Parser { s: text, pos: 0 };
    // RapidJSON skips a UTF-8 BOM only with AutoUTFInputStream; plain MemoryStream does not.
    p.skip_whitespace();
    if p.pos >= p.s.len() {
        return p.err("The document is empty.", p.pos);
    }
    let v = p.parse_value()?;
    p.skip_whitespace();
    if p.pos < p.s.len() {
        return p.err("The document root must not be followed by other values.", p.pos);
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numbers_like_rapidjson() {
        let v = parse(br#"{"a": 0.4, "b": -3, "c": 4294967296, "d": 1e3, "e": [1, 2.5, "x"], "a": 7}"#).unwrap();
        assert_eq!(v.get("a"), Some(&JsonValue::Double(0.4)));
        assert_eq!(v.get("b"), Some(&JsonValue::Int(-3)));
        assert_eq!(v.get("c"), Some(&JsonValue::Uint(4294967296)));
        assert_eq!(v.get("d"), Some(&JsonValue::Double(1000.0)));
        assert_eq!(v.members().len(), 6);
    }
}
