//! A small XML reader for 3MF model files: elements with attributes and text, namespace prefixes
//! stripped from element names (attribute names keep their prefix).

#[derive(Clone, Debug, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    pub fn attr_any(&self, names: &[&str]) -> Option<&str> {
        names.iter().find_map(|n| self.attr(n)).or_else(|| {
            // Any prefix: match on the local part.
            self.attrs.iter().find(|(k, _)| names.iter().any(|n| k.rsplit(':').next() == n.rsplit(':').next())).map(|(_, v)| v.as_str())
        })
    }
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        let after = &rest[p..];
        if let Some(e) = after.find(';') {
            let ent = &after[1..e];
            let rep = match ent {
                "lt" => Some('<'),
                "gt" => Some('>'),
                "amp" => Some('&'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if ent.starts_with("#x") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
                _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            if let Some(c) = rep {
                out.push(c);
                rest = &after[e + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &after[1..];
    }
    out.push_str(rest);
    out
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn parse(text: &str) -> Result<Element, String> {
    let b = text.as_bytes();
    let mut i = 0;
    let mut stack: Vec<Element> = vec![Element::default()];
    while i < b.len() {
        if b[i] == b'<' {
            if text[i..].starts_with("<?") {
                i = text[i..].find("?>").map(|p| i + p + 2).ok_or("unterminated declaration")?;
                continue;
            }
            if text[i..].starts_with("<!--") {
                i = text[i..].find("-->").map(|p| i + p + 3).ok_or("unterminated comment")?;
                continue;
            }
            if text[i..].starts_with("<![CDATA[") {
                let end = text[i..].find("]]>").map(|p| i + p).ok_or("unterminated CDATA")?;
                stack.last_mut().unwrap().text.push_str(&text[i + 9..end]);
                i = end + 3;
                continue;
            }
            if text[i..].starts_with("<!") {
                i = text[i..].find('>').map(|p| i + p + 1).ok_or("unterminated doctype")?;
                continue;
            }
            if text[i..].starts_with("</") {
                let end = text[i..].find('>').map(|p| i + p).ok_or("unterminated end tag")?;
                let e = stack.pop().ok_or("unbalanced")?;
                stack.last_mut().ok_or("unbalanced end tag")?.children.push(e);
                i = end + 1;
                continue;
            }
            // Start tag.
            let mut j = i + 1;
            while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' && b[j] != b'/' {
                j += 1;
            }
            let mut e = Element { name: local(&text[i + 1..j]), ..Default::default() };
            loop {
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j >= b.len() {
                    return Err("unterminated start tag".into());
                }
                if b[j] == b'>' {
                    stack.push(e);
                    j += 1;
                    break;
                }
                if b[j] == b'/' {
                    stack.last_mut().unwrap().children.push(e);
                    j = text[j..].find('>').map(|p| j + p + 1).ok_or("bad empty tag")?;
                    break;
                }
                let ks = j;
                while j < b.len() && b[j] != b'=' && !b[j].is_ascii_whitespace() {
                    j += 1;
                }
                let key = text[ks..j].to_string();
                while j < b.len() && (b[j] == b'=' || b[j].is_ascii_whitespace()) {
                    j += 1;
                }
                if j >= b.len() {
                    return Err("bad attribute".into());
                }
                let q = b[j];
                if q != b'"' && q != b'\'' {
                    return Err("unquoted attribute".into());
                }
                let ve = text[j + 1..].find(q as char).map(|p| j + 1 + p).ok_or("unterminated attribute")?;
                e.attrs.push((key, unescape(&text[j + 1..ve])));
                j = ve + 1;
            }
            i = j;
        } else {
            let end = text[i..].find('<').map(|p| i + p).unwrap_or(b.len());
            let t = unescape(&text[i..end]);
            let top = stack.last_mut().unwrap();
            if !t.trim().is_empty() || !top.text.is_empty() {
                top.text.push_str(&t);
            }
            i = end;
        }
    }
    let mut root = stack.pop().ok_or("empty document")?;
    if !stack.is_empty() {
        return Err("unclosed elements".into());
    }
    let mut r = std::mem::take(&mut root.children).into_iter().next().ok_or_else(|| "no root element".to_string())?;
    r.text = r.text.trim().to_string();
    Ok(r)
}
