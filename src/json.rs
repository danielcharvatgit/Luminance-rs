//! Minimal JSON, replacing `serde_json`: just enough for the D-Bus
//! `GetMonitors` reply and `--list-displays --json`. Output matches what
//! serde_json produced (integers without a fraction, floats always with one,
//! non-finite floats as null).

use std::fmt::{self, Write};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<Value>),
    /// Key order is preserved.
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn object<const N: usize>(pairs: [(&str, Value); N]) -> Self {
        Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// Object member lookup; `None` for missing keys and non-objects.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Value::Int(n) => Some(n as f64),
            Value::Float(n) => Some(n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn parse(text: &str) -> Result<Value, String> {
        let mut p = Parser { s: text.as_bytes(), i: 0 };
        let value = p.value(0)?;
        p.ws();
        if p.i != p.s.len() {
            return Err(p.err("trailing characters"));
        }
        Ok(value)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<usize> for Value {
    fn from(n: usize) -> Self {
        Value::Int(n as i64)
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Float(n)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(o: Option<T>) -> Self {
        o.map_or(Value::Null, Into::into)
    }
}

impl From<Vec<Value>> for Value {
    fn from(items: Vec<Value>) -> Self {
        Value::Array(items)
    }
}

fn write_str(f: &mut fmt::Formatter<'_>, s: &str) -> fmt::Result {
    f.write_char('"')?;
    for c in s.chars() {
        match c {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\r' => f.write_str("\\r")?,
            '\t' => f.write_str("\\t")?,
            c if (c as u32) < 0x20 => write!(f, "\\u{:04x}", c as u32)?,
            c => f.write_char(c)?,
        }
    }
    f.write_char('"')
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => f.write_str("null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(n) => write!(f, "{n}"),
            // Debug formatting keeps the ".0" and uses exponents for extremes,
            // both valid JSON.
            Value::Float(n) if n.is_finite() => write!(f, "{n:?}"),
            Value::Float(_) => f.write_str("null"),
            Value::String(s) => write_str(f, s),
            Value::Array(items) => {
                f.write_char('[')?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_char(',')?;
                    }
                    write!(f, "{v}")?;
                }
                f.write_char(']')
            }
            Value::Object(members) => {
                f.write_char('{')?;
                for (i, (k, v)) in members.iter().enumerate() {
                    if i > 0 {
                        f.write_char(',')?;
                    }
                    write_str(f, k)?;
                    write!(f, ":{v}")?;
                }
                f.write_char('}')
            }
        }
    }
}

/// Nesting limit, so hostile input cannot overflow the stack.
const MAX_DEPTH: usize = 64;

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, what: &str) -> String {
        format!("invalid JSON at byte {}: {what}", self.i)
    }

    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|b| b.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }

    fn expect(&mut self, b: u8) -> Result<(), String> {
        if self.peek() == Some(b) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.err(&format!("expected '{}'", b as char)))
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(value)
        } else {
            Err(self.err("unknown literal"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err(self.err("nested too deeply"));
        }
        match self.peek() {
            Some(b'{') => {
                self.i += 1;
                let mut members = Vec::new();
                if self.peek() == Some(b'}') {
                    self.i += 1;
                    return Ok(Value::Object(members));
                }
                loop {
                    if self.peek() != Some(b'"') {
                        return Err(self.err("expected object key"));
                    }
                    let key = self.string()?;
                    self.expect(b':')?;
                    members.push((key, self.value(depth + 1)?));
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Value::Object(members));
                        }
                        _ => return Err(self.err("expected ',' or '}'")),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                if self.peek() == Some(b']') {
                    self.i += 1;
                    return Ok(Value::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Value::Array(items));
                        }
                        _ => return Err(self.err("expected ',' or ']'")),
                    }
                }
            }
            Some(b'"') => self.string().map(Value::String),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.err("expected a value")),
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.i;
        let mut float = false;
        while let Some(&b) = self.s.get(self.i) {
            match b {
                b'0'..=b'9' | b'-' | b'+' => {}
                b'.' | b'e' | b'E' => float = true,
                _ => break,
            }
            self.i += 1;
        }
        // Only ASCII bytes were consumed, so this slice is valid UTF-8.
        let text = std::str::from_utf8(&self.s[start..self.i]).unwrap();
        let int = if float { None } else { text.parse().ok().map(Value::Int) };
        int.or_else(|| text.parse().ok().map(Value::Float)).ok_or_else(|| self.err("bad number"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self.s.get(self.i..self.i + 4).ok_or_else(|| self.err("short \\u escape"))?;
        let code = std::str::from_utf8(digits)
            .ok()
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .ok_or_else(|| self.err("bad \\u escape"))?;
        self.i += 4;
        Ok(code)
    }

    fn string(&mut self) -> Result<String, String> {
        self.i += 1; // opening quote
        let mut out = String::new();
        loop {
            // Copy the run up to the next quote or backslash in one go.
            let run = self.s[self.i..].iter().position(|&b| b == b'"' || b == b'\\');
            let Some(len) = run else { return Err(self.err("unterminated string")) };
            let chunk = std::str::from_utf8(&self.s[self.i..self.i + len]).map_err(|_| self.err("invalid UTF-8"))?;
            out.push_str(chunk);
            self.i += len + 1;
            if self.s[self.i - 1] == b'"' {
                return Ok(out);
            }
            let esc = *self.s.get(self.i).ok_or_else(|| self.err("unterminated escape"))?;
            self.i += 1;
            match esc {
                b'"' => out.push('"'),
                b'\\' => out.push('\\'),
                b'/' => out.push('/'),
                b'b' => out.push('\u{8}'),
                b'f' => out.push('\u{c}'),
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                b'u' => {
                    let mut code = self.hex4()?;
                    // UTF-16 surrogate pair.
                    if (0xD800..0xDC00).contains(&code) && self.s[self.i..].starts_with(b"\\u") {
                        self.i += 2;
                        let low = self.hex4()?;
                        code = 0x10000 + ((code - 0xD800) << 10) + (low.wrapping_sub(0xDC00) & 0x3FF);
                    }
                    out.push(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
                }
                _ => return Err(self.err("bad escape")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_like_serde_json() {
        let v = Value::object([
            ("n", 1usize.into()),
            ("f", 40.0.into()),
            ("g", 0.46.into()),
            ("none", None::<f64>.into()),
            ("nan", f64::NAN.into()),
            ("s", "a\"b\\c\n\u{1}".into()),
            ("list", vec![true.into(), Value::Null].into()),
        ]);
        assert_eq!(
            v.to_string(),
            r#"{"n":1,"f":40.0,"g":0.46,"none":null,"nan":null,"s":"a\"b\\c\n\u0001","list":[true,null]}"#
        );
    }

    #[test]
    fn round_trips() {
        let v = Value::object([("name", "DELL U2415".into()), ("b", 70.5.into()), ("x", (-1.0).into())]);
        assert_eq!(Value::parse(&v.to_string()).unwrap(), v);
    }

    #[test]
    fn parses() {
        let v = Value::parse(r#" [ {"a" : -1, "b": 2.5e1, "c": "é😀\/", "d": [], "e": {}} ] "#).unwrap();
        let m = &v.as_array().unwrap()[0];
        assert_eq!(m.get("a"), Some(&Value::Int(-1)));
        assert_eq!(m.get("a").and_then(Value::as_f64), Some(-1.0));
        assert_eq!(m.get("b").and_then(Value::as_f64), Some(25.0));
        assert_eq!(m.get("c").and_then(Value::as_str), Some("é😀/"));
        assert_eq!(m.get("d"), Some(&Value::Array(vec![])));
        assert_eq!(m.get("missing"), None);
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["", "{", "[1,]", "{\"a\"}", "tru", "\"abc", "1 2", "{1:2}", &"[".repeat(100)] {
            assert!(Value::parse(bad).is_err(), "{bad:?}");
        }
    }
}
