//! Non-executing parser for PQS's Python-literal metadata dialect.
use crate::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    List(Vec<Value>),
    Object(BTreeMap<String, Value>),
}
impl Value {
    pub fn string(&self) -> Option<&str> {
        if let Self::String(s) = self {
            Some(s)
        } else {
            None
        }
    }
    pub fn object(&self) -> Option<&BTreeMap<String, Value>> {
        if let Self::Object(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn u64(&self) -> Option<u64> {
        if let Self::Number(s) = self {
            s.parse().ok()
        } else {
            None
        }
    }
    pub fn json(&self) -> String {
        match self {
            Self::Null => "null".into(),
            Self::Bool(b) => b.to_string(),
            Self::Number(n) => n.clone(),
            Self::String(s) => {
                let mut out = String::from("\"");
                for c in s.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
                        c => out.push(c),
                    }
                }
                out.push('"');
                out
            }
            Self::List(v) => format!(
                "[{}]",
                v.iter().map(Self::json).collect::<Vec<_>>().join(",")
            ),
            Self::Object(v) => format!(
                "{{{}}}",
                v.iter()
                    .map(|(k, v)| format!("{}:{}", Self::from(k.as_str()).json(), v.json()))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }
}
impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Self::String(s.into())
    }
}
impl From<u64> for Value {
    fn from(n: u64) -> Self {
        Self::Number(n.to_string())
    }
}
pub(crate) fn obj(items: impl IntoIterator<Item = (impl Into<String>, Value)>) -> Value {
    Value::Object(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

struct Parser {
    chars: Vec<char>,
    at: usize,
}
impl Parser {
    fn ws(&mut self) {
        while self.chars.get(self.at).is_some_and(|c| c.is_whitespace()) {
            self.at += 1;
        }
    }
    fn eat(&mut self, c: char) -> bool {
        self.ws();
        if self.chars.get(self.at) == Some(&c) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn quoted(&mut self) -> Result<String> {
        let quote = self.chars[self.at];
        self.at += 1;
        let mut out = String::new();
        while let Some(&c) = self.chars.get(self.at) {
            self.at += 1;
            if c == quote {
                return Ok(out);
            }
            if c != '\\' {
                ensure!(c != '\n' && c != '\r', "newline in metadata string");
                out.push(c);
                continue;
            }
            let escape = *self
                .chars
                .get(self.at)
                .context("unfinished string escape")?;
            self.at += 1;
            match escape {
                '\\' | '\'' | '"' => out.push(escape),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'a' => out.push('\u{7}'),
                'v' => out.push('\u{b}'),
                'x' | 'u' | 'U' => {
                    let n = match escape {
                        'x' => 2,
                        'u' => 4,
                        _ => 8,
                    };
                    let end = self.at + n;
                    ensure!(end <= self.chars.len(), "short Unicode escape");
                    let digits: String = self.chars[self.at..end].iter().collect();
                    let cp = u32::from_str_radix(&digits, 16).context("invalid Unicode escape")?;
                    out.push(char::from_u32(cp).context("invalid Unicode scalar")?);
                    self.at = end;
                }
                _ => bail!("unsupported metadata string escape"),
            }
        }
        bail!("unterminated metadata string")
    }
    fn value(&mut self, depth: usize) -> Result<Value> {
        ensure!(depth <= 64, "metadata nesting exceeds 64");
        self.ws();
        let c = *self
            .chars
            .get(self.at)
            .context("unexpected end of metadata")?;
        if c == '\'' || c == '"' {
            return self.quoted().map(Value::String);
        }
        if self.eat('{') {
            let mut fields = BTreeMap::new();
            if self.eat('}') {
                return Ok(Value::Object(fields));
            }
            loop {
                self.ws();
                ensure!(
                    matches!(self.chars.get(self.at), Some('\'' | '"')),
                    "metadata keys must be quoted strings"
                );
                let key = self.quoted()?;
                ensure!(self.eat(':'), "expected colon in metadata");
                let val = self.value(depth + 1)?;
                ensure!(fields.insert(key, val).is_none(), "duplicate metadata key");
                if self.eat('}') {
                    break;
                }
                ensure!(self.eat(','), "expected metadata comma");
                if self.eat('}') {
                    break;
                }
            }
            return Ok(Value::Object(fields));
        }
        if self.eat('[') {
            let mut vals = vec![];
            if self.eat(']') {
                return Ok(Value::List(vals));
            }
            loop {
                vals.push(self.value(depth + 1)?);
                if self.eat(']') {
                    break;
                }
                ensure!(self.eat(','), "expected list comma");
                if self.eat(']') {
                    break;
                }
            }
            return Ok(Value::List(vals));
        }
        let start = self.at;
        while self
            .chars
            .get(self.at)
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '-'))
        {
            self.at += 1;
        }
        ensure!(
            self.at > start,
            "unsupported metadata expression at character {}",
            self.at
        );
        let token: String = self.chars[start..self.at].iter().collect();
        match token.as_str() {
            "True" => Ok(Value::Bool(true)),
            "False" => Ok(Value::Bool(false)),
            "None" => Ok(Value::Null),
            _ => {
                let dtype = token
                    .strip_prefix("pl.")
                    .or_else(|| token.strip_prefix("polars."))
                    .unwrap_or(&token);
                if [
                    "String",
                    "Utf8",
                    "Categorical",
                    "UInt8",
                    "UInt16",
                    "UInt32",
                    "UInt64",
                    "Int8",
                    "Int16",
                    "Int32",
                    "Int64",
                    "Float32",
                    "Float64",
                    "Boolean",
                ]
                .contains(&dtype)
                {
                    return Ok(Value::from(dtype));
                }
                if let Ok(n) = token.parse::<u64>() {
                    return Ok(Value::from(n));
                }
                if let Ok(n) = token.parse::<i64>() {
                    return Ok(Value::Number(n.to_string()));
                }
                if let Ok(n) = token.parse::<f64>() {
                    ensure!(n.is_finite(), "nonfinite metadata number");
                    return Ok(Value::Number(n.to_string()));
                }
                bail!("unsupported metadata token: {token}")
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Metadata {
    pub fields: BTreeMap<String, Value>,
}
impl Metadata {
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.len() <= 16 * 1024 * 1024,
            "metadata exceeds 16 MiB limit"
        );
        let mut parser = Parser {
            chars: text.chars().collect(),
            at: 0,
        };
        let value = parser.value(0)?;
        parser.ws();
        ensure!(
            parser.at == parser.chars.len(),
            "trailing metadata expression"
        );
        match value {
            Value::Object(fields) => Ok(Self { fields }),
            _ => bail!("metadata must be a dictionary"),
        }
    }
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        use std::io::Read;
        let mut text = String::new();
        File::open(path.as_ref().join("_metadata"))?
            .take(16 * 1024 * 1024 + 1)
            .read_to_string(&mut text)?;
        Self::parse(&text)
    }
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(Value::string)
    }
    pub fn kind(&self) -> Result<Kind> {
        ensure!(
            self.fields.get("is_pqs") == Some(&Value::Bool(true)),
            "not a PQS dataset"
        );
        match self.get_str("format") {
            Some("pairs") => Ok(Kind::Pairs),
            Some("concat") => Ok(Kind::Concat),
            _ => bail!("unsupported PQS format"),
        }
    }
    pub fn supported_kind(&self) -> Result<Kind> {
        let kind = self.kind()?;
        ensure!(
            self.get_str("format-version")
                == Some(if kind == Kind::Pairs {
                    "0.1.0"
                } else {
                    "0.2.0"
                }),
            "unsupported PQS format version"
        );
        Ok(kind)
    }
    /// Missing scope retains the legacy Reader's global-ID interpretation.
    pub fn shard_scoped(&self) -> bool {
        self.get_str("read_idx_scope") == Some("shard")
    }
    pub fn to_value(&self) -> Value {
        Value::Object(self.fields.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restricted_parser() {
        let m = Metadata::parse("{'x': [pl.UInt64, '中文\\u0021', None, -1, 1.25], 'a': True,}")
            .unwrap();
        assert!(m.to_value().json().contains("中文!"));
        for s in [
            "{'x': __import__('os')}",
            "{'x': pl.UInt64()}",
            "{'a': 1, 'a': 2}",
            "{} garbage",
            "{'x': NaN}",
        ] {
            assert!(Metadata::parse(s).is_err(), "{s}");
        }
    }
}
