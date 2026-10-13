//! `utils/string.h`: efficient number to string conversions used in the G-code output, plus
//! emulations of the `std::ostream` default and `std::fixed` floating point formatting.

use std::fmt::Write;

/// `writeInt2mm`: efficient conversion of micron integer type to millimeter string.
///
/// Note: like the C++ code, the value is first narrowed to `int32_t`.
pub fn write_int2mm(coord: i64, ss: &mut String) {
    let coord = coord as i32;
    let mut buffer: Vec<u8> = coord.to_string().into_bytes();
    let char_count = buffer.len() as i32;
    let mut trailing_zeros: i32 = 1;
    while trailing_zeros < 4 && char_count - trailing_zeros >= 0 && buffer[(char_count - trailing_zeros) as usize] == b'0' {
        trailing_zeros += 1;
    }
    trailing_zeros -= 1;
    let end_pos = char_count - trailing_zeros;
    if trailing_zeros == 3 {
        // no need to write the decimal dot
        buffer.truncate((char_count - trailing_zeros) as usize);
        ss.push_str(std::str::from_utf8(&buffer).unwrap());
        return;
    }
    if char_count <= 3 {
        let mut start = 0usize; // where to start writing from the buffer
        if coord < 0 {
            ss.push('-');
            start = 1;
        }
        ss.push_str("0.");
        let mut nulls = char_count - start as i32;
        while nulls < 3 {
            // fill up to 3 decimals with zeros
            ss.push('0');
            nulls += 1;
        }
        buffer.truncate((char_count - trailing_zeros) as usize);
        ss.push_str(std::str::from_utf8(&buffer[start..]).unwrap());
    } else {
        // shift all characters and insert the decimal dot
        let mut out: Vec<u8> = buffer[..(char_count - 3) as usize].to_vec();
        out.push(b'.');
        let mut pos = char_count - 3;
        while pos < end_pos {
            out.push(buffer[pos as usize]);
            pos += 1;
        }
        ss.push_str(std::str::from_utf8(&out).unwrap());
    }
}

/// `MMtoStream` as a string.
pub fn mm_to_string(coord: i64) -> String {
    let mut s = String::new();
    write_int2mm(coord, &mut s);
    s
}

/// `writeDoubleToStream`: writes with `precision` digits after the decimal dot, but removes trailing zeros.
pub fn write_double_to_stream(precision: u8, coord: f64, ss: &mut String) {
    let buffer = if coord.is_nan() {
        if coord.is_sign_negative() {
            "-NAN".to_string()
        } else {
            "NAN".to_string()
        }
    } else if coord.is_infinite() {
        if coord < 0.0 {
            "-INF".to_string()
        } else {
            "INF".to_string()
        }
    } else {
        format!("{:.*}", precision as usize, coord)
    };
    let bytes = buffer.as_bytes();
    let char_count = bytes.len() as i64;
    if char_count <= 0 {
        return;
    }
    let dot_idx = char_count - precision as i64 - 1;
    if dot_idx >= 0 && bytes[dot_idx as usize] == b'.' {
        let mut non_nul_pos = char_count - 1;
        while bytes[non_nul_pos as usize] == b'0' {
            non_nul_pos -= 1;
        }
        if bytes[non_nul_pos as usize] == b'.' {
            ss.push_str(&buffer[..non_nul_pos as usize]);
        } else {
            ss.push_str(&buffer[..(non_nul_pos + 1) as usize]);
        }
        return;
    }
    ss.push_str(&buffer);
}

/// `PrecisionedDouble`
#[derive(Clone, Copy, Debug)]
pub struct PrecisionedDouble {
    /// Number of digits after the decimal mark with which to convert to string
    pub precision: u8,
    /// The double value
    pub value: f64,
}

impl PrecisionedDouble {
    pub fn new(precision: u8, value: f64) -> Self {
        PrecisionedDouble { precision, value }
    }

    pub fn would_write_zero(&self) -> bool {
        (self.value.abs() * 10f64.powi(self.precision as i32)) < 1.0
    }
}

impl std::fmt::Display for PrecisionedDouble {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = String::new();
        write_double_to_stream(self.precision, self.value, &mut s);
        f.write_str(&s)
    }
}

/// `MMtoStream`
#[derive(Clone, Copy, Debug)]
pub struct MMtoStream(pub i64);

impl std::fmt::Display for MMtoStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = String::new();
        write_int2mm(self.0, &mut s);
        f.write_str(&s)
    }
}

/// Formats a double the way a default `std::ostream` does (`%g` with precision 6).
pub fn fmt_g(value: f64) -> String {
    fmt_g_prec(value, 6)
}

/// `printf("%.{precision}g", value)`.
pub fn fmt_g_prec(value: f64, precision: usize) -> String {
    if value.is_nan() {
        return if value.is_sign_negative() { "-nan".into() } else { "nan".into() };
    }
    if value.is_infinite() {
        return if value < 0.0 { "-inf".into() } else { "inf".into() };
    }
    let p = if precision == 0 { 1 } else { precision };
    if value == 0.0 {
        return if value.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    // Determine the exponent X after rounding to p significant digits.
    let e_str = format!("{:.*e}", p - 1, value);
    let (mantissa, exp) = e_str.split_once('e').unwrap();
    let x: i32 = exp.parse().unwrap();
    if (p as i32) > x && x >= -4 {
        let decimals = (p as i32 - 1 - x) as usize;
        let s = format!("{:.*}", decimals, value);
        strip_trailing_zeros(&s)
    } else {
        let m = strip_trailing_zeros(mantissa);
        let mut out = String::new();
        out.push_str(&m);
        out.push('e');
        if x < 0 {
            out.push('-');
        } else {
            out.push('+');
        }
        let _ = write!(out, "{:02}", x.abs());
        out
    }
}

fn strip_trailing_zeros(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }
    let t = s.trim_end_matches('0');
    let t = t.trim_end_matches('.');
    t.to_string()
}

/// Formats a double the way a `std::ostream` with `std::fixed` does (`%f`, precision 6).
pub fn fmt_fixed(value: f64) -> String {
    if value.is_nan() {
        return if value.is_sign_negative() { "-nan".into() } else { "nan".into() };
    }
    if value.is_infinite() {
        return if value < 0.0 { "-inf".into() } else { "inf".into() };
    }
    format!("{:.6}", value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int2mm() {
        let cases: [(i64, &str); 21] = [
            (0, "0.00"),
            (-100, "-.1"),
            (-1000, "-1"),
            (10, "0.01"),
            (-10, "-0.01"),
            (1, "0.001"),
            (-1, "-0.001"),
            (1001, "1.001"),
            (12, "0.012"),
            (2147483652, "-2147483.644"),
            (5, "0.005"),
            (-5, "-0.005"),
            (-50, "-0.05"),
            (100, "0.1"),
            (1000, "1"),
            (1234, "1.234"),
            (-1234, "-1.234"),
            (99969, "99.969"),
            (99500, "99.5"),
            (120000, "120"),
            (300, "0.3"),
        ];
        for (v, e) in cases {
            assert_eq!(mm_to_string(v), e, "value {}", v);
        }
    }

    #[test]
    fn precisioned() {
        assert_eq!(PrecisionedDouble::new(1, 1500.0).to_string(), "1500");
        assert_eq!(PrecisionedDouble::new(5, 1279.004514).to_string(), "1279.00451");
        assert_eq!(PrecisionedDouble::new(5, -6.5).to_string(), "-6.5");
        assert_eq!(PrecisionedDouble::new(1, 210.0).to_string(), "210");
        assert_eq!(PrecisionedDouble::new(0, 500.0).to_string(), "500");
    }

    #[test]
    fn general() {
        assert_eq!(fmt_g(2147483.647), "2.14748e+06");
        assert_eq!(fmt_g(-2147483.647), "-2.14748e+06");
        assert_eq!(fmt_g(0.1), "0.1");
        assert_eq!(fmt_g(0.4), "0.4");
        assert_eq!(fmt_g(96.5), "96.5");
        assert_eq!(fmt_g(1e-5), "1e-05");
        assert_eq!(fmt_g(123456.0), "123456");
        assert_eq!(fmt_g(1234567.0), "1.23457e+06");
        assert_eq!(fmt_g(0.0001), "0.0001");
    }
}
