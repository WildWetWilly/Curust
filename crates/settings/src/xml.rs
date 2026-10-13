//! Minimal XML reader (ElementTree-like) for material profiles: elements with namespace-resolved
//! tags (`{uri}local`), attributes, text and children. Comments, processing instructions and
//! doctypes are skipped.

#[derive(Clone, Debug, Default)]
pub struct Element {
    /// `{namespace}local` or `local`.
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    /// Text before the first child (`element.text`); `None` if there is none.
    pub text: Option<String>,
    pub children: Vec<Element>,
}

impl Element {
    pub fn local_name(&self) -> &str {
        self.tag.rsplit('}').next().unwrap_or(&self.tag)
    }
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    /// Children with the given namespace URI and local name.
    pub fn find_all<'a>(&'a self, ns: &'a str, local: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.tag.strip_prefix('{').and_then(|r| r.split_once('}')).map(|(n, l)| n == ns && l == local).unwrap_or(false))
    }
    pub fn find<'a>(&'a self, ns: &'a str, local: &'a str) -> Option<&'a Element> {
        self.find_all(ns, local).next()
    }
    pub fn namespace(&self) -> &str {
        self.tag.strip_prefix('{').and_then(|r| r.split_once('}')).map(|(n, _)| n).unwrap_or("")
    }
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if let Some(j) = rest.find(';') {
            let ent = &rest[1..j];
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
                rest = &rest[j + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

struct RawElem {
    qname: String,
    attrs: Vec<(String, String)>,
    text: Option<String>,
    children: Vec<RawElem>,
}

pub fn parse(text: &str) -> Result<Element, String> {
    let b = text.as_bytes();
    let mut i = 0;
    let mut stack: Vec<RawElem> = Vec::new();
    let mut root: Option<RawElem> = None;
    while i < b.len() {
        if b[i] == b'<' {
            if text[i..].starts_with("<!--") {
                let end = text[i..].find("-->").ok_or("unterminated comment")?;
                i += end + 3;
                continue;
            }
            if text[i..].starts_with("<?") {
                let end = text[i..].find("?>").ok_or("unterminated processing instruction")?;
                i += end + 2;
                continue;
            }
            if text[i..].starts_with("<![CDATA[") {
                let end = text[i..].find("]]>").ok_or("unterminated CDATA")?;
                let data = &text[i + 9..i + end];
                if let Some(top) = stack.last_mut() {
                    append_text(top, data);
                }
                i += end + 3;
                continue;
            }
            if text[i..].starts_with("<!") {
                let end = text[i..].find('>').ok_or("unterminated declaration")?;
                i += end + 1;
                continue;
            }
            if text[i..].starts_with("</") {
                let end = text[i..].find('>').ok_or("unterminated end tag")?;
                let name = text[i + 2..i + end].trim();
                let el = stack.pop().ok_or("unexpected end tag")?;
                if el.qname != name {
                    return Err(format!("mismatched end tag {} for {}", name, el.qname));
                }
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(el);
                } else {
                    root = Some(el);
                }
                i += end + 1;
                continue;
            }
            // Start tag.
            let mut j = i + 1;
            while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' && b[j] != b'/' {
                j += 1;
            }
            let qname = text[i + 1..j].to_string();
            let mut attrs = Vec::new();
            loop {
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j >= b.len() {
                    return Err("unterminated start tag".into());
                }
                if b[j] == b'>' || b[j] == b'/' {
                    break;
                }
                let ns = j;
                while j < b.len() && b[j] != b'=' && !b[j].is_ascii_whitespace() {
                    j += 1;
                }
                let an = text[ns..j].to_string();
                while j < b.len() && (b[j].is_ascii_whitespace() || b[j] == b'=') {
                    j += 1;
                }
                let q = b[j];
                if q != b'"' && q != b'\'' {
                    return Err("bad attribute".into());
                }
                let vs = j + 1;
                let ve = vs + text[vs..].find(q as char).ok_or("unterminated attribute")?;
                attrs.push((an, unescape(&text[vs..ve])));
                j = ve + 1;
            }
            let self_closing = b[j] == b'/';
            let close = j + text[j..].find('>').ok_or("unterminated tag")?;
            let el = RawElem { qname, attrs, text: None, children: Vec::new() };
            if self_closing {
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(el);
                } else {
                    root = Some(el);
                }
            } else {
                stack.push(el);
            }
            i = close + 1;
        } else {
            let end = text[i..].find('<').map(|e| i + e).unwrap_or(b.len());
            if let Some(top) = stack.last_mut() {
                let t = unescape(&text[i..end]);
                append_text(top, &t);
            }
            i = end;
        }
    }
    let root = root.ok_or("no root element")?;
    Ok(resolve(root, &[]))
}

fn append_text(el: &mut RawElem, t: &str) {
    // ElementTree: text before the first child belongs to `.text`; later text is a child's tail.
    if el.children.is_empty() {
        match &mut el.text {
            Some(s) => s.push_str(t),
            None => el.text = Some(t.to_string()),
        }
    }
}

fn resolve(el: RawElem, scopes: &[(String, String)]) -> Element {
    let mut scope: Vec<(String, String)> = scopes.to_vec();
    let mut attrs = Vec::new();
    for (k, v) in &el.attrs {
        if k == "xmlns" {
            scope.push((String::new(), v.clone()));
        } else if let Some(p) = k.strip_prefix("xmlns:") {
            scope.push((p.to_string(), v.clone()));
        }
    }
    for (k, v) in el.attrs {
        if k == "xmlns" || k.starts_with("xmlns:") {
            continue;
        }
        let name = match k.split_once(':') {
            Some((p, l)) => match scope.iter().rev().find(|(sp, _)| sp == p) {
                Some((_, uri)) => format!("{{{}}}{}", uri, l),
                None => k.clone(),
            },
            None => k,
        };
        attrs.push((name, v));
    }
    let tag = match el.qname.split_once(':') {
        Some((p, l)) => match scope.iter().rev().find(|(sp, _)| sp == p) {
            Some((_, uri)) => format!("{{{}}}{}", uri, l),
            None => el.qname.clone(),
        },
        None => match scope.iter().rev().find(|(sp, _)| sp.is_empty()) {
            Some((_, uri)) if !uri.is_empty() => format!("{{{}}}{}", uri, el.qname),
            _ => el.qname.clone(),
        },
    };
    let children = el.children.into_iter().map(|c| resolve(c, &scope)).collect();
    Element { tag, attrs, text: el.text, children }
}

/// ElementTree's text escaping (`_escape_cdata`).
fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// ElementTree's attribute escaping (`_escape_attrib`).
fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\r', "&#13;").replace('\n', "&#10;").replace('\t', "&#09;")
}

/// Write an element tree the way Cura saves material profiles: `<?xml ...?>` declaration,
/// namespace prefixes from `namespaces` (URI → prefix, "" for the default namespace, declared on
/// the root), two-space indentation (`XmlMaterialProfile._indent`) and `<tag />` for empty
/// elements.
pub fn write_document(root: &Element, namespaces: &[(&str, &str)]) -> String {
    fn qname(tag: &str, ns: &[(&str, &str)]) -> String {
        if let Some((uri, local)) = tag.strip_prefix('{').and_then(|r| r.split_once('}')) {
            match ns.iter().find(|(u, _)| *u == uri) {
                Some((_, "")) => local.to_string(),
                Some((_, p)) => format!("{}:{}", p, local),
                None => local.to_string(),
            }
        } else {
            tag.to_string()
        }
    }
    fn write(e: &Element, level: usize, ns: &[(&str, &str)], out: &mut String, root: bool) {
        let name = qname(&e.tag, ns);
        out.push('<');
        out.push_str(&name);
        if root {
            for (uri, p) in ns {
                if p.is_empty() {
                    out.push_str(&format!(" xmlns=\"{}\"", escape_attr(uri)));
                } else {
                    out.push_str(&format!(" xmlns:{}=\"{}\"", p, escape_attr(uri)));
                }
            }
        }
        for (k, v) in &e.attrs {
            out.push_str(&format!(" {}=\"{}\"", qname(k, ns), escape_attr(v)));
        }
        let text = e.text.as_deref().filter(|t| !t.trim().is_empty() || e.children.is_empty());
        if e.children.is_empty() {
            match text {
                Some(t) if !t.is_empty() => {
                    out.push('>');
                    out.push_str(&escape_text(t));
                    out.push_str(&format!("</{}>", name));
                }
                _ => out.push_str(" />"),
            }
            return;
        }
        out.push('>');
        match text {
            Some(t) => out.push_str(&escape_text(t)),
            None => {
                out.push('\n');
                out.push_str(&"  ".repeat(level + 1));
            }
        }
        for (i, c) in e.children.iter().enumerate() {
            write(c, level + 1, ns, out, false);
            out.push('\n');
            if i + 1 < e.children.len() {
                out.push_str(&"  ".repeat(level + 1));
            } else {
                out.push_str(&"  ".repeat(level));
            }
        }
        out.push_str(&format!("</{}>", name));
    }
    let mut out = String::from("<?xml version='1.0' encoding='utf-8'?>\n");
    write(root, 0, namespaces, &mut out, true);
    out
}

impl Element {
    pub fn new(tag: &str) -> Element {
        Element { tag: tag.to_string(), ..Default::default() }
    }
    pub fn find_mut<'a>(&'a mut self, ns: &str, local: &str) -> Option<&'a mut Element> {
        self.children.iter_mut().find(|c| c.tag.strip_prefix('{').and_then(|r| r.split_once('}')).map(|(n, l)| n == ns && l == local).unwrap_or(false))
    }
    /// The child `{ns}local`, created (appended) when missing.
    pub fn child_mut(&mut self, ns: &str, local: &str) -> &mut Element {
        if self.find_mut(ns, local).is_none() {
            self.children.push(Element::new(&format!("{{{}}}{}", ns, local)));
        }
        self.find_mut(ns, local).unwrap()
    }
    pub fn set_attr(&mut self, k: &str, v: &str) {
        if let Some(slot) = self.attrs.iter_mut().find(|(x, _)| x == k) {
            slot.1 = v.to_string();
        } else {
            self.attrs.push((k.to_string(), v.to_string()));
        }
    }
}
