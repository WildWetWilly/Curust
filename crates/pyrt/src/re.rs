//! The `re` module: a backtracking regular expression engine with Python's syntax and
//! semantics (leftmost-first alternation, greedy/lazy/possessive quantifiers, groups, named
//! groups, back-references, look-around, inline flags, Python 3.7+ empty-match rules).

use crate::ast::Name;
use crate::builtins::{builtin, to_int};
use crate::interp::{Flow, Interp, R};
use crate::modules::make_module;
use crate::value::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub const I: u32 = 2;
pub const L: u32 = 4;
pub const M: u32 = 8;
pub const S: u32 = 16;
pub const U: u32 = 32;
pub const X: u32 = 64;
pub const A: u32 = 256;

// ---------------------------------------------------------------------- character tests

fn is_word(c: char, ascii: bool) -> bool {
    if ascii {
        c.is_ascii_alphanumeric() || c == '_'
    } else {
        c.is_alphanumeric() || c == '_'
    }
}

fn is_digit(c: char, ascii: bool) -> bool {
    if ascii {
        c.is_ascii_digit()
    } else {
        c.is_ascii_digit() || (c as u32 > 127 && c.is_numeric() && !c.is_alphabetic() && unicode_decimal(c))
    }
}

/// Unicode decimal digits (category Nd) outside ASCII: approximate with the common blocks.
fn unicode_decimal(c: char) -> bool {
    let u = c as u32;
    let starts = [0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66, 0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0xff10];
    starts.iter().any(|&s| u >= s && u < s + 10)
}

fn is_space(c: char, ascii: bool) -> bool {
    if ascii {
        matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
    } else {
        crate::strings::py_isspace(c)
    }
}

fn lower(c: char) -> char {
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

fn upper(c: char) -> char {
    let mut l = c.to_uppercase();
    match (l.next(), l.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

#[derive(Clone, Debug)]
enum ClassItem {
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
}

#[derive(Debug)]
struct CharClass {
    negated: bool,
    items: Vec<ClassItem>,
    icase: bool,
    ascii: bool,
}

impl CharClass {
    fn test1(&self, c: char) -> bool {
        self.items.iter().any(|it| match it {
            ClassItem::Range(a, b) => *a <= c && c <= *b,
            ClassItem::Digit(neg) => is_digit(c, self.ascii) != *neg,
            ClassItem::Word(neg) => is_word(c, self.ascii) != *neg,
            ClassItem::Space(neg) => is_space(c, self.ascii) != *neg,
        })
    }
    fn matches(&self, c: char) -> bool {
        let hit = self.test1(c) || (self.icase && (self.test1(lower(c)) || self.test1(upper(c))));
        hit != self.negated
    }
}

#[derive(Clone, Debug)]
enum Item {
    Char(char),
    /// Case-insensitive; holds the lower-case form.
    CharI(char),
    /// `.` without DOTALL.
    AnyNoNl,
    /// `.` with DOTALL.
    Any,
    Class(Rc<CharClass>),
}

impl Item {
    #[inline]
    fn matches(&self, c: char) -> bool {
        match self {
            Item::Char(x) => *x == c,
            Item::CharI(x) => *x == lower(c) || *x == c,
            Item::AnyNoNl => c != '\n',
            Item::Any => true,
            Item::Class(cc) => cc.matches(c),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Assert {
    /// `^` without MULTILINE, `\A`
    Start,
    /// `^` with MULTILINE
    StartLine,
    /// `$` without MULTILINE (end, or before a final newline)
    End,
    /// `$` with MULTILINE
    EndLine,
    /// `\Z`
    StrEnd,
    WordB(bool),
    NotWordB(bool),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Greed {
    Greedy,
    Lazy,
    Possessive,
}

#[derive(Debug)]
enum Node {
    Empty,
    Item(Item),
    Assert(Assert),
    Group(Box<Node>, Option<usize>),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat(Box<Node>, usize, usize, Greed),
    Backref(usize, bool),
    Look(Box<Node>, bool, bool),
    Atomic(Box<Node>),
}

impl Node {
    fn width(&self) -> Option<usize> {
        match self {
            Node::Empty | Node::Assert(_) | Node::Look(..) => Some(0),
            Node::Item(_) => Some(1),
            Node::Group(n, _) | Node::Atomic(n) => n.width(),
            Node::Concat(v) => v.iter().map(|n| n.width()).sum(),
            Node::Alt(v) => {
                let w = v.first()?.width()?;
                if v.iter().all(|n| n.width() == Some(w)) {
                    Some(w)
                } else {
                    None
                }
            }
            Node::Repeat(n, a, b, _) if a == b => n.width().map(|w| w * a),
            _ => None,
        }
    }
    fn nullable(&self) -> bool {
        match self {
            Node::Empty | Node::Assert(_) | Node::Look(..) | Node::Backref(..) => true,
            Node::Item(_) => false,
            Node::Group(n, _) | Node::Atomic(n) => n.nullable(),
            Node::Concat(v) => v.iter().all(|n| n.nullable()),
            Node::Alt(v) => v.iter().any(|n| n.nullable()),
            Node::Repeat(n, min, _, _) => *min == 0 || n.nullable(),
        }
    }
    /// The single item of a node (for the fast repeat instruction).
    fn single_item(&self) -> Option<Item> {
        match self {
            Node::Item(it) => Some(it.clone()),
            Node::Group(n, None) => n.single_item(),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------- parser

struct Parser {
    c: Vec<char>,
    k: usize,
    ngroups: usize,
    names: Vec<(String, usize)>,
    open: Vec<usize>,
}

type PR<T> = Result<T, String>;

impl Parser {
    fn peek(&self) -> Option<char> {
        self.c.get(self.k).copied()
    }
    fn eat(&mut self, ch: char) -> bool {
        if self.peek() == Some(ch) {
            self.k += 1;
            true
        } else {
            false
        }
    }
    fn skip_verbose(&mut self, flags: u32) {
        if flags & X == 0 {
            return;
        }
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => self.k += 1,
                Some('#') => {
                    while let Some(c) = self.peek() {
                        self.k += 1;
                        if c == '\n' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    fn parse_alt(&mut self, flags: &mut u32) -> PR<Node> {
        let mut branches = vec![self.parse_concat(flags)?];
        while self.eat('|') {
            branches.push(self.parse_concat(flags)?);
        }
        Ok(if branches.len() == 1 { branches.pop().unwrap() } else { Node::Alt(branches) })
    }

    fn parse_concat(&mut self, flags: &mut u32) -> PR<Node> {
        let mut items = Vec::new();
        loop {
            self.skip_verbose(*flags);
            match self.peek() {
                None | Some('|') | Some(')') => break,
                _ => {}
            }
            let atom = self.parse_atom(flags)?;
            let atom = self.parse_quant(atom, *flags)?;
            items.push(atom);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().unwrap(),
            _ => Node::Concat(items),
        })
    }

    fn parse_number(&mut self) -> Option<usize> {
        let start = self.k;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.k += 1;
        }
        if self.k == start {
            None
        } else {
            self.c[start..self.k].iter().collect::<String>().parse().ok()
        }
    }

    fn parse_quant(&mut self, atom: Node, flags: u32) -> PR<Node> {
        let mut atom = atom;
        loop {
            self.skip_verbose(flags);
            let (min, max) = match self.peek() {
                Some('*') => {
                    self.k += 1;
                    (0, usize::MAX)
                }
                Some('+') => {
                    self.k += 1;
                    (1, usize::MAX)
                }
                Some('?') => {
                    self.k += 1;
                    (0, 1)
                }
                Some('{') => {
                    let save = self.k;
                    self.k += 1;
                    let lo = self.parse_number();
                    let r = if self.eat(',') {
                        let hi = self.parse_number();
                        if self.eat('}') {
                            Some((lo.unwrap_or(0), hi.unwrap_or(usize::MAX)))
                        } else {
                            None
                        }
                    } else if lo.is_some() && self.eat('}') {
                        Some((lo.unwrap(), lo.unwrap()))
                    } else {
                        None
                    };
                    match r {
                        Some(r) => {
                            if r.1 < r.0 {
                                return Err("min repeat greater than max repeat".into());
                            }
                            r
                        }
                        None => {
                            self.k = save;
                            return Ok(atom);
                        }
                    }
                }
                _ => return Ok(atom),
            };
            if matches!(atom, Node::Repeat(..)) && !matches!(atom, Node::Repeat(_, _, _, Greed::Possessive)) {
                // `a**` is an error in Python; `a*?` was consumed below.
            }
            if matches!(atom, Node::Empty | Node::Assert(_)) {
                return Err("nothing to repeat".into());
            }
            let greed = if self.eat('?') {
                Greed::Lazy
            } else if self.eat('+') {
                Greed::Possessive
            } else {
                Greed::Greedy
            };
            if let Node::Repeat(..) = atom {
                return Err("multiple repeat".into());
            }
            atom = Node::Repeat(Box::new(atom), min, max, greed);
        }
    }

    fn parse_flags_letters(&mut self) -> (u32, u32) {
        let mut on = 0;
        let mut off = 0;
        let mut neg = false;
        while let Some(c) = self.peek() {
            let f = match c {
                'i' => I,
                'L' => L,
                'm' => M,
                's' => S,
                'u' => U,
                'x' => X,
                'a' => A,
                '-' => {
                    neg = true;
                    self.k += 1;
                    continue;
                }
                _ => break,
            };
            if neg {
                off |= f;
            } else {
                on |= f;
            }
            self.k += 1;
        }
        (on, off)
    }

    fn parse_atom(&mut self, flags: &mut u32) -> PR<Node> {
        let c = self.peek().unwrap();
        self.k += 1;
        let icase = *flags & I != 0;
        let ascii = *flags & A != 0;
        Ok(match c {
            '.' => Node::Item(if *flags & S != 0 { Item::Any } else { Item::AnyNoNl }),
            '^' => Node::Assert(if *flags & M != 0 { Assert::StartLine } else { Assert::Start }),
            '$' => Node::Assert(if *flags & M != 0 { Assert::EndLine } else { Assert::End }),
            '[' => Node::Item(Item::Class(Rc::new(self.parse_class(*flags)?))),
            '(' => {
                if self.eat('?') {
                    match self.peek() {
                        Some(':') => {
                            self.k += 1;
                            let mut f = *flags;
                            let n = self.parse_alt(&mut f)?;
                            self.expect_close()?;
                            Node::Group(Box::new(n), None)
                        }
                        Some('P') | Some('<') if self.peek() == Some('P') || (self.c.get(self.k + 1) != Some(&'=') && self.c.get(self.k + 1) != Some(&'!')) => {
                            let is_p = self.peek() == Some('P');
                            if is_p {
                                self.k += 1;
                            }
                            if is_p && self.eat('=') {
                                // (?P=name)
                                let start = self.k;
                                while matches!(self.peek(), Some(c) if c != ')') {
                                    self.k += 1;
                                }
                                let name: String = self.c[start..self.k].iter().collect();
                                self.expect_close()?;
                                let idx = self.names.iter().find(|(n, _)| *n == name).map(|(_, i)| *i).ok_or_else(|| format!("unknown group name '{}'", name))?;
                                return Ok(Node::Backref(idx, icase));
                            }
                            if !self.eat('<') {
                                return Err("unknown extension ?P".into());
                            }
                            let start = self.k;
                            while matches!(self.peek(), Some(c) if c != '>') {
                                self.k += 1;
                            }
                            let name: String = self.c[start..self.k].iter().collect();
                            if !self.eat('>') {
                                return Err("missing >, unterminated name".into());
                            }
                            if name.is_empty() || !(name.chars().next().unwrap().is_alphabetic() || name.starts_with('_')) || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                                return Err(format!("bad character in group name '{}'", name));
                            }
                            if self.names.iter().any(|(n, _)| *n == name) {
                                return Err(format!("redefinition of group name '{}'", name));
                            }
                            self.ngroups += 1;
                            let idx = self.ngroups;
                            self.names.push((name, idx));
                            self.open.push(idx);
                            let mut f = *flags;
                            let n = self.parse_alt(&mut f)?;
                            self.open.pop();
                            self.expect_close()?;
                            Node::Group(Box::new(n), Some(idx))
                        }
                        Some('=') | Some('!') => {
                            let neg = self.peek() == Some('!');
                            self.k += 1;
                            let mut f = *flags;
                            let n = self.parse_alt(&mut f)?;
                            self.expect_close()?;
                            Node::Look(Box::new(n), true, neg)
                        }
                        Some('<') => {
                            self.k += 1;
                            let neg = match self.peek() {
                                Some('=') => false,
                                Some('!') => true,
                                _ => return Err("unknown extension ?<".into()),
                            };
                            self.k += 1;
                            let mut f = *flags;
                            let n = self.parse_alt(&mut f)?;
                            self.expect_close()?;
                            if n.width().is_none() {
                                return Err("look-behind requires fixed-width pattern".into());
                            }
                            Node::Look(Box::new(n), false, neg)
                        }
                        Some('>') => {
                            self.k += 1;
                            let mut f = *flags;
                            let n = self.parse_alt(&mut f)?;
                            self.expect_close()?;
                            Node::Atomic(Box::new(n))
                        }
                        Some('#') => {
                            while matches!(self.peek(), Some(c) if c != ')') {
                                self.k += 1;
                            }
                            self.expect_close()?;
                            Node::Empty
                        }
                        Some(_) => {
                            let (on, off) = self.parse_flags_letters();
                            if self.eat(')') {
                                // Global (or rest-of-group) flags.
                                *flags = (*flags | on) & !off;
                                Node::Empty
                            } else if self.eat(':') {
                                let mut f = (*flags | on) & !off;
                                let n = self.parse_alt(&mut f)?;
                                self.expect_close()?;
                                Node::Group(Box::new(n), None)
                            } else {
                                return Err("unknown extension".into());
                            }
                        }
                        None => return Err("unexpected end of pattern".into()),
                    }
                } else {
                    self.ngroups += 1;
                    let idx = self.ngroups;
                    self.open.push(idx);
                    let mut f = *flags;
                    let n = self.parse_alt(&mut f)?;
                    self.open.pop();
                    self.expect_close()?;
                    Node::Group(Box::new(n), Some(idx))
                }
            }
            ')' => return Err("unbalanced parenthesis".into()),
            '*' | '+' | '?' => return Err("nothing to repeat".into()),
            '{' => {
                // A brace that doesn't start a valid quantifier is a literal.
                let save = self.k;
                let lo = self.parse_number();
                let ok = if self.eat(',') {
                    self.parse_number();
                    self.peek() == Some('}')
                } else {
                    lo.is_some() && self.peek() == Some('}')
                };
                self.k = save;
                if ok {
                    return Err("nothing to repeat".into());
                }
                self.literal('{', icase)
            }
            '\\' => self.parse_escape(icase, ascii, false)?.0,
            c => self.literal(c, icase),
        })
    }

    fn literal(&self, c: char, icase: bool) -> Node {
        if icase && (lower(c) != c || upper(c) != c) {
            Node::Item(Item::CharI(lower(c)))
        } else {
            Node::Item(Item::Char(c))
        }
    }

    fn expect_close(&mut self) -> PR<()> {
        if self.eat(')') {
            Ok(())
        } else {
            Err("missing ), unterminated subpattern".into())
        }
    }

    fn hex(&mut self, n: usize) -> PR<char> {
        let s: String = self.c.get(self.k..self.k + n).ok_or("incomplete escape")?.iter().collect();
        if s.len() != n || !s.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("incomplete escape \\{}", s));
        }
        self.k += n;
        char::from_u32(u32::from_str_radix(&s, 16).unwrap()).ok_or_else(|| "bad escape".to_string())
    }

    /// An escape after the backslash. Returns the node and, in classes, the literal char.
    fn parse_escape(&mut self, icase: bool, ascii: bool, in_class: bool) -> PR<(Node, Option<char>)> {
        let Some(c) = self.peek() else {
            return Err("bad escape (end of pattern)".into());
        };
        self.k += 1;
        let cls = |item: ClassItem| -> Node { Node::Item(Item::Class(Rc::new(CharClass { negated: false, items: vec![item], icase: false, ascii }))) };
        let lit = |ch: char, p: &Parser| -> (Node, Option<char>) { (p.literal(ch, icase), Some(ch)) };
        Ok(match c {
            'd' => (cls(ClassItem::Digit(false)), None),
            'D' => (cls(ClassItem::Digit(true)), None),
            'w' => (cls(ClassItem::Word(false)), None),
            'W' => (cls(ClassItem::Word(true)), None),
            's' => (cls(ClassItem::Space(false)), None),
            'S' => (cls(ClassItem::Space(true)), None),
            'A' if !in_class => (Node::Assert(Assert::Start), None),
            'Z' if !in_class => (Node::Assert(Assert::StrEnd), None),
            'b' if !in_class => (Node::Assert(Assert::WordB(ascii)), None),
            'B' if !in_class => (Node::Assert(Assert::NotWordB(ascii)), None),
            'b' => lit('\x08', self),
            'a' => lit('\x07', self),
            'f' => lit('\x0c', self),
            'n' => lit('\n', self),
            'r' => lit('\r', self),
            't' => lit('\t', self),
            'v' => lit('\x0b', self),
            'x' => {
                let ch = self.hex(2)?;
                lit(ch, self)
            }
            'u' => {
                let ch = self.hex(4)?;
                lit(ch, self)
            }
            'U' => {
                let ch = self.hex(8)?;
                lit(ch, self)
            }
            '0' => {
                let mut v = 0u32;
                let mut n = 0;
                while n < 2 && matches!(self.peek(), Some(d) if ('0'..='7').contains(&d)) {
                    v = v * 8 + self.peek().unwrap().to_digit(8).unwrap();
                    self.k += 1;
                    n += 1;
                }
                lit(char::from_u32(v).unwrap(), self)
            }
            '1'..='9' => {
                let mut digits = c.to_string();
                if matches!(self.peek(), Some(d) if d.is_ascii_digit()) {
                    digits.push(self.peek().unwrap());
                    self.k += 1;
                    let d: Vec<char> = digits.chars().collect();
                    if ('0'..='7').contains(&d[0]) && ('0'..='7').contains(&d[1]) && matches!(self.peek(), Some(e) if ('0'..='7').contains(&e)) {
                        digits.push(self.peek().unwrap());
                        self.k += 1;
                        let v = u32::from_str_radix(&digits, 8).unwrap();
                        if v > 0o377 {
                            return Err(format!("octal escape value \\{} outside of range 0-0o377", digits));
                        }
                        return Ok(lit(char::from_u32(v).unwrap(), self));
                    }
                }
                if in_class {
                    // Octal in classes.
                    match u32::from_str_radix(&digits, 8) {
                        Ok(v) => return Ok(lit(char::from_u32(v).unwrap(), self)),
                        Err(_) => return Err(format!("bad escape \\{}", digits)),
                    }
                }
                let g: usize = digits.parse().unwrap();
                if g > self.ngroups {
                    return Err(format!("invalid group reference {}", g));
                }
                if self.open.contains(&g) {
                    return Err("cannot refer to an open group".into());
                }
                (Node::Backref(g, icase), None)
            }
            c if c.is_ascii_alphabetic() => return Err(format!("bad escape \\{}", c)),
            c => lit(c, self),
        })
    }

    fn parse_class(&mut self, flags: u32) -> PR<CharClass> {
        let ascii = flags & A != 0;
        let mut cc = CharClass { negated: false, items: Vec::new(), icase: flags & I != 0, ascii };
        if self.eat('^') {
            cc.negated = true;
        }
        let mut first = true;
        loop {
            let Some(c) = self.peek() else {
                return Err("unterminated character set".into());
            };
            if c == ']' && !first {
                self.k += 1;
                break;
            }
            first = false;
            self.k += 1;
            let lo: Option<char> = if c == '\\' {
                let (node, ch) = self.parse_escape(false, ascii, true)?;
                match ch {
                    Some(ch) => Some(ch),
                    None => {
                        if let Node::Item(Item::Class(inner)) = node {
                            cc.items.extend(inner.items.iter().cloned());
                        }
                        continue;
                    }
                }
            } else {
                Some(c)
            };
            let lo = lo.unwrap();
            // Range?
            if self.peek() == Some('-') && self.c.get(self.k + 1).is_some_and(|&n| n != ']') {
                self.k += 1;
                let hc = self.peek().unwrap();
                self.k += 1;
                let hi = if hc == '\\' {
                    let (_, ch) = self.parse_escape(false, ascii, true)?;
                    ch.ok_or_else(|| "bad character range".to_string())?
                } else {
                    hc
                };
                if hi < lo {
                    return Err(format!("bad character range {}-{}", lo, hi));
                }
                cc.items.push(ClassItem::Range(lo, hi));
            } else {
                cc.items.push(ClassItem::Range(lo, lo));
            }
        }
        Ok(cc)
    }
}

// ---------------------------------------------------------------------- compiler

#[derive(Debug)]
enum Inst {
    Item(Item),
    Assert(Assert),
    Save(usize),
    Split(usize, usize),
    Jmp(usize),
    Mark(usize),
    Progress(usize),
    Backref(usize, bool),
    Look { prog: usize, behind: Option<usize>, negate: bool },
    Atomic(usize),
    Repeat { item: Item, min: usize, max: usize, greedy: bool },
    Match,
}

pub struct Regex {
    progs: Vec<Vec<Inst>>,
    pub ngroups: usize,
    pub names: Vec<(String, usize)>,
    pub flags: u32,
    pub pattern: String,
    nregs: usize,
    first: Option<char>,
    anchored: bool,
}

struct Compiler {
    progs: Vec<Vec<Inst>>,
    nregs: usize,
}

impl Compiler {
    fn emit(&mut self, p: usize, i: Inst) -> usize {
        self.progs[p].push(i);
        self.progs[p].len() - 1
    }
    fn here(&self, p: usize) -> usize {
        self.progs[p].len()
    }
    fn sub(&mut self, n: &Node) -> PR<usize> {
        self.progs.push(Vec::new());
        let p = self.progs.len() - 1;
        self.node(p, n)?;
        self.emit(p, Inst::Match);
        Ok(p)
    }
    fn node(&mut self, p: usize, n: &Node) -> PR<()> {
        match n {
            Node::Empty => {}
            Node::Item(it) => {
                self.emit(p, Inst::Item(it.clone()));
            }
            Node::Assert(a) => {
                self.emit(p, Inst::Assert(*a));
            }
            Node::Group(inner, cap) => {
                if let Some(g) = cap {
                    self.emit(p, Inst::Save(2 * g));
                    self.node(p, inner)?;
                    self.emit(p, Inst::Save(2 * g + 1));
                } else {
                    self.node(p, inner)?;
                }
            }
            Node::Concat(v) => {
                for x in v {
                    self.node(p, x)?;
                }
            }
            Node::Alt(v) => {
                let mut jumps = Vec::new();
                for (k, b) in v.iter().enumerate() {
                    if k + 1 < v.len() {
                        let split = self.emit(p, Inst::Split(0, 0));
                        let start = self.here(p);
                        self.node(p, b)?;
                        jumps.push(self.emit(p, Inst::Jmp(0)));
                        let next = self.here(p);
                        self.progs[p][split] = Inst::Split(start, next);
                    } else {
                        self.node(p, b)?;
                    }
                }
                let end = self.here(p);
                for j in jumps {
                    self.progs[p][j] = Inst::Jmp(end);
                }
            }
            Node::Backref(g, ic) => {
                self.emit(p, Inst::Backref(*g, *ic));
            }
            Node::Look(inner, ahead, neg) => {
                let sp = self.sub(inner)?;
                let behind = if *ahead { None } else { Some(inner.width().unwrap_or(0)) };
                self.emit(p, Inst::Look { prog: sp, behind, negate: *neg });
            }
            Node::Atomic(inner) => {
                let sp = self.sub(inner)?;
                self.emit(p, Inst::Atomic(sp));
            }
            Node::Repeat(inner, min, max, greed) => {
                if *greed == Greed::Possessive {
                    let sp = self.sub(&Node::Repeat(Box::new(clone_node(inner)), *min, *max, Greed::Greedy))?;
                    self.emit(p, Inst::Atomic(sp));
                    return Ok(());
                }
                let greedy = *greed == Greed::Greedy;
                if let Some(item) = inner.single_item() {
                    self.emit(p, Inst::Repeat { item, min: *min, max: *max, greedy });
                    return Ok(());
                }
                if *min > 2000 || (*max != usize::MAX && *max > 2000) {
                    return Err("repetition count too large for this engine".into());
                }
                for _ in 0..*min {
                    self.node(p, inner)?;
                }
                let nullable = inner.nullable();
                if *max == usize::MAX {
                    let l1 = self.emit(p, Inst::Split(0, 0));
                    let l2 = self.here(p);
                    let reg = if nullable {
                        let r = self.nregs;
                        self.nregs += 1;
                        self.emit(p, Inst::Mark(r));
                        Some(r)
                    } else {
                        None
                    };
                    self.node(p, inner)?;
                    if let Some(r) = reg {
                        self.emit(p, Inst::Progress(r));
                    }
                    self.emit(p, Inst::Jmp(l1));
                    let l3 = self.here(p);
                    self.progs[p][l1] = if greedy { Inst::Split(l2, l3) } else { Inst::Split(l3, l2) };
                } else {
                    let mut splits = Vec::new();
                    for _ in *min..*max {
                        let s = self.emit(p, Inst::Split(0, 0));
                        splits.push((s, self.here(p)));
                        self.node(p, inner)?;
                    }
                    let end = self.here(p);
                    for (s, body) in splits {
                        self.progs[p][s] = if greedy { Inst::Split(body, end) } else { Inst::Split(end, body) };
                    }
                }
            }
        }
        Ok(())
    }
}

fn clone_node(n: &Node) -> Node {
    match n {
        Node::Empty => Node::Empty,
        Node::Item(i) => Node::Item(i.clone()),
        Node::Assert(a) => Node::Assert(*a),
        Node::Group(x, c) => Node::Group(Box::new(clone_node(x)), *c),
        Node::Concat(v) => Node::Concat(v.iter().map(clone_node).collect()),
        Node::Alt(v) => Node::Alt(v.iter().map(clone_node).collect()),
        Node::Repeat(x, a, b, g) => Node::Repeat(Box::new(clone_node(x)), *a, *b, *g),
        Node::Backref(g, i) => Node::Backref(*g, *i),
        Node::Look(x, a, b) => Node::Look(Box::new(clone_node(x)), *a, *b),
        Node::Atomic(x) => Node::Atomic(Box::new(clone_node(x))),
    }
}

impl Regex {
    pub fn new(pattern: &str, flags: u32) -> Result<Regex, String> {
        let mut p = Parser { c: pattern.chars().collect(), k: 0, ngroups: 0, names: Vec::new(), open: Vec::new() };
        let mut f = flags;
        // Leading global inline flags.
        while p.c.get(p.k) == Some(&'(') && p.c.get(p.k + 1) == Some(&'?') && p.c.get(p.k + 2).is_some_and(|c| "aiLmsux".contains(*c)) {
            let save = p.k;
            p.k += 2;
            let (on, off) = p.parse_flags_letters();
            if p.eat(')') {
                f = (f | on) & !off;
            } else {
                p.k = save;
                break;
            }
        }
        let node = p.parse_alt(&mut f)?;
        if p.k < p.c.len() {
            return Err("unbalanced parenthesis".into());
        }
        let mut c = Compiler { progs: vec![Vec::new()], nregs: 0 };
        c.node(0, &node)?;
        c.emit(0, Inst::Match);
        // A literal first character lets the search skip ahead.
        let mut first = None;
        let mut anchored = false;
        for inst in &c.progs[0] {
            match inst {
                Inst::Save(_) => continue,
                Inst::Item(Item::Char(ch)) => first = Some(*ch),
                Inst::Repeat { item: Item::Char(ch), min, .. } if *min > 0 => first = Some(*ch),
                Inst::Assert(Assert::Start) => anchored = true,
                _ => {}
            }
            break;
        }
        Ok(Regex { progs: c.progs, ngroups: p.ngroups, names: p.names, flags: f, pattern: pattern.to_string(), nregs: c.nregs, first, anchored })
    }

    fn check_assert(a: Assert, text: &[char], pos: usize) -> bool {
        let n = text.len();
        match a {
            Assert::Start => pos == 0,
            Assert::StartLine => pos == 0 || text[pos - 1] == '\n',
            Assert::End => pos == n || (pos + 1 == n && text[pos] == '\n'),
            Assert::EndLine => pos == n || text[pos] == '\n',
            Assert::StrEnd => pos == n,
            Assert::WordB(ascii) | Assert::NotWordB(ascii) => {
                let before = pos > 0 && is_word(text[pos - 1], ascii);
                let after = pos < n && is_word(text[pos], ascii);
                (before != after) == matches!(a, Assert::WordB(_))
            }
        }
    }

    /// Run program `p` anchored at `start`. On success returns the end position.
    #[allow(clippy::too_many_arguments)]
    fn run(&self, p: usize, text: &[char], start: usize, caps: &mut [Option<usize>], regs: &mut [usize], required_end: Option<usize>, no_empty_at: Option<usize>, budget: &mut u64) -> Result<Option<usize>, String> {
        enum Bt {
            Alt(usize, usize),
            Slot(usize, Option<usize>),
            Reg(usize, usize),
            RepG { pc: usize, min_pos: usize, pos: usize },
            RepL { inst: usize, pos: usize, count: usize },
        }
        let prog = &self.progs[p];
        let n = text.len();
        let mut stack: Vec<Bt> = Vec::new();
        let mut pc = 0usize;
        let mut pos = start;
        loop {
            *budget = budget.saturating_sub(1);
            if *budget == 0 {
                return Err("regular expression is too complex for this input (catastrophic backtracking)".into());
            }
            let ok = match &prog[pc] {
                Inst::Item(it) => {
                    if pos < n && it.matches(text[pos]) {
                        pos += 1;
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::Assert(a) => {
                    if Self::check_assert(*a, text, pos) {
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::Save(s) => {
                    stack.push(Bt::Slot(*s, caps[*s]));
                    caps[*s] = Some(pos);
                    pc += 1;
                    true
                }
                Inst::Split(x, y) => {
                    stack.push(Bt::Alt(*y, pos));
                    pc = *x;
                    true
                }
                Inst::Jmp(x) => {
                    pc = *x;
                    true
                }
                Inst::Mark(r) => {
                    stack.push(Bt::Reg(*r, regs[*r]));
                    regs[*r] = pos;
                    pc += 1;
                    true
                }
                Inst::Progress(r) => {
                    if regs[*r] == pos {
                        false
                    } else {
                        pc += 1;
                        true
                    }
                }
                Inst::Backref(g, icase) => match (caps[2 * g], caps[2 * g + 1]) {
                    (Some(a), Some(b)) if b >= a => {
                        let len = b - a;
                        if pos + len <= n && (0..len).all(|k| if *icase { lower(text[a + k]) == lower(text[pos + k]) } else { text[a + k] == text[pos + k] }) {
                            pos += len;
                            pc += 1;
                            true
                        } else {
                            false
                        }
                    }
                    _ => false,
                },
                Inst::Look { prog: sp, behind, negate } => {
                    let (from, req) = match behind {
                        None => (Some(pos), None),
                        Some(w) => (if pos >= *w { Some(pos - w) } else { None }, Some(pos)),
                    };
                    let found = match from {
                        Some(f) => {
                            let mut sub_caps = caps.to_vec();
                            let mut sub_regs = regs.to_vec();
                            let r = self.run(*sp, text, f, &mut sub_caps, &mut sub_regs, req, None, budget)?;
                            if r.is_some() && !negate {
                                for (k, v) in sub_caps.iter().enumerate() {
                                    if caps[k] != *v {
                                        stack.push(Bt::Slot(k, caps[k]));
                                        caps[k] = *v;
                                    }
                                }
                            }
                            r.is_some()
                        }
                        None => false,
                    };
                    if found != *negate {
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::Atomic(sp) => {
                    let mut sub_caps = caps.to_vec();
                    let mut sub_regs = regs.to_vec();
                    match self.run(*sp, text, pos, &mut sub_caps, &mut sub_regs, None, None, budget)? {
                        Some(e) => {
                            for (k, v) in sub_caps.iter().enumerate() {
                                if caps[k] != *v {
                                    stack.push(Bt::Slot(k, caps[k]));
                                    caps[k] = *v;
                                }
                            }
                            pos = e;
                            pc += 1;
                            true
                        }
                        None => false,
                    }
                }
                Inst::Repeat { item, min, max, greedy } => {
                    if *greedy {
                        let mut k = 0;
                        while k < *max && pos + k < n && item.matches(text[pos + k]) {
                            k += 1;
                        }
                        if k < *min {
                            false
                        } else {
                            if k > *min {
                                stack.push(Bt::RepG { pc: pc + 1, min_pos: pos + min, pos: pos + k });
                            }
                            pos += k;
                            pc += 1;
                            true
                        }
                    } else {
                        let mut k = 0;
                        while k < *min && pos + k < n && item.matches(text[pos + k]) {
                            k += 1;
                        }
                        if k < *min {
                            false
                        } else {
                            stack.push(Bt::RepL { inst: pc, pos: pos + k, count: k });
                            pos += k;
                            pc += 1;
                            true
                        }
                    }
                }
                Inst::Match => {
                    let end_ok = required_end.is_none_or(|e| e == pos);
                    let empty_ok = no_empty_at != Some(pos) || pos != start;
                    if end_ok && empty_ok {
                        return Ok(Some(pos));
                    }
                    false
                }
            };
            if ok {
                continue;
            }
            // Backtrack.
            loop {
                match stack.pop() {
                    None => return Ok(None),
                    Some(Bt::Alt(x, ps)) => {
                        pc = x;
                        pos = ps;
                        break;
                    }
                    Some(Bt::Slot(s, old)) => caps[s] = old,
                    Some(Bt::Reg(r, old)) => regs[r] = old,
                    Some(Bt::RepG { pc: x, min_pos, pos: ps }) => {
                        let np = ps - 1;
                        if np > min_pos {
                            stack.push(Bt::RepG { pc: x, min_pos, pos: np });
                        }
                        pc = x;
                        pos = np;
                        break;
                    }
                    Some(Bt::RepL { inst, pos: ps, count }) => {
                        if let Inst::Repeat { item, max, .. } = &prog[inst] {
                            if count < *max && ps < n && item.matches(text[ps]) {
                                stack.push(Bt::RepL { inst, pos: ps + 1, count: count + 1 });
                                pc = inst + 1;
                                pos = ps + 1;
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Search `text[..endpos]` from `pos`. `anchored` = match only at `pos`; `full` = must end at endpos.
    pub fn search_at(&self, text: &[char], pos: usize, anchored: bool, full: bool, must_advance: bool) -> Result<Option<Vec<Option<usize>>>, String> {
        let n = text.len();
        let mut budget: u64 = 400_000_000;
        let ncap = 2 * (self.ngroups + 1);
        let mut caps = vec![None; ncap];
        let mut regs = vec![usize::MAX; self.nregs];
        let mut s = pos;
        while s <= n {
            if self.anchored && s != 0 && !anchored {
                return Ok(None);
            }
            if let (Some(c), false) = (self.first, anchored) {
                // Skip to the next occurrence of the literal first character.
                match text[s..].iter().position(|&x| x == c) {
                    Some(off) => s += off,
                    None => return Ok(None),
                }
            }
            for c in caps.iter_mut() {
                *c = None;
            }
            for r in regs.iter_mut() {
                *r = usize::MAX;
            }
            let no_empty = if must_advance { Some(pos) } else { None };
            if let Some(e) = self.run(0, text, s, &mut caps, &mut regs, if full { Some(n) } else { None }, if s == pos { no_empty } else { None }, &mut budget)? {
                caps[0] = Some(s);
                caps[1] = Some(e);
                return Ok(Some(caps));
            }
            if anchored {
                return Ok(None);
            }
            s += 1;
        }
        Ok(None)
    }
}

// ---------------------------------------------------------------------- Python objects

thread_local! {
    static CACHE: RefCell<HashMap<(String, u32), Rc<Regex>>> = RefCell::new(HashMap::new());
}

fn compile_cached(i: &mut Interp, pattern: &str, flags: u32) -> R<Rc<Regex>> {
    if let Some(r) = CACHE.with(|c| c.borrow().get(&(pattern.to_string(), flags)).cloned()) {
        return Ok(r);
    }
    match Regex::new(pattern, flags) {
        Ok(r) => {
            let r = Rc::new(r);
            CACHE.with(|c| {
                let mut c = c.borrow_mut();
                if c.len() > 512 {
                    c.clear();
                }
                c.insert((pattern.to_string(), flags), r.clone());
            });
            Ok(r)
        }
        Err(e) => Err(re_error(i, &format!("{} at position 0", e))),
    }
}

fn re_error(i: &mut Interp, msg: &str) -> Flow {
    let class = match i.types.exceptions.get("re.error") {
        Some(c) => c.clone(),
        None => {
            let c = new_class("error", vec![i.exc_class("Exception")], "Exception");
            i.types.exceptions.insert("re.error", c.clone());
            c
        }
    };
    let inst = Instance { class, dict: RefCell::new(Namespace::default()), base: RefCell::new(Value::tuple(vec![Value::str(msg)])) };
    Flow::Exc(Value::Instance(Rc::new(inst)))
}

pub struct PatternObj {
    pub re: Rc<Regex>,
}

pub struct MatchObj {
    re: Rc<Regex>,
    pattern: Value,
    string: Value,
    text: Rc<Vec<char>>,
    caps: Vec<Option<usize>>,
    pos: usize,
    endpos: usize,
}

impl MatchObj {
    fn group_index(&self, i: &mut Interp, g: &Value) -> R<usize> {
        match g.unwrap_base() {
            Value::Int(n) if n >= 0 && (n as usize) <= self.re.ngroups => Ok(n as usize),
            Value::Bool(b) => Ok(b as usize),
            Value::Str(s) => match self.re.names.iter().find(|(n, _)| **n == *s) {
                Some((_, k)) => Ok(*k),
                None => Err(i.exc("IndexError", "no such group")),
            },
            _ => Err(i.exc("IndexError", "no such group")),
        }
    }
    fn group_str(&self, k: usize) -> Value {
        match (self.caps[2 * k], self.caps[2 * k + 1]) {
            (Some(a), Some(b)) if b >= a => Value::Str(self.text[a..b].iter().collect::<String>().into()),
            _ => Value::None,
        }
    }
    fn span(&self, k: usize) -> (i64, i64) {
        match (self.caps[2 * k], self.caps[2 * k + 1]) {
            (Some(a), Some(b)) => (a as i64, b as i64),
            _ => (-1, -1),
        }
    }
}

fn str_of(i: &mut Interp, v: &Value) -> R<Rc<str>> {
    match v.unwrap_base() {
        Value::Str(s) => Ok(s),
        Value::Bytes(b) => Ok(b.iter().map(|&c| c as char).collect::<String>().into()),
        other => i.type_error(format!("expected string or bytes-like object, got '{}'", other.type_name())),
    }
}

fn pattern_of(i: &mut Interp, v: &Value, flags: u32) -> R<(Rc<Regex>, Value)> {
    if let Value::Native(n) = v {
        if let Some(p) = n.as_any().downcast_ref::<PatternObj>() {
            if flags != 0 {
                return i.value_error("cannot process flags argument with a compiled pattern");
            }
            return Ok((p.re.clone(), v.clone()));
        }
    }
    let s = str_of(i, v)?;
    let re = compile_cached(i, &s, flags)?;
    Ok((re.clone(), Value::Native(Rc::new(PatternObj { re }))))
}

fn kw_or(kw: &[(Name, Value)], name: &str) -> Option<Value> {
    kw.iter().find(|(k, _)| &**k == name).map(|(_, v)| v.clone())
}

fn flags_arg(i: &mut Interp, v: Option<Value>) -> R<u32> {
    match v {
        None | Some(Value::None) => Ok(0),
        Some(v) => Ok(to_int(i, &v)? as u32),
    }
}

/// Run a search and wrap the result.
fn do_search(i: &mut Interp, re: &Rc<Regex>, pat: &Value, string: &Value, pos: usize, endpos: Option<usize>, anchored: bool, full: bool) -> R<Value> {
    let s = str_of(i, string)?;
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let endpos = endpos.unwrap_or(n).min(n);
    let text: Rc<Vec<char>> = Rc::new(if endpos < n { chars[..endpos].to_vec() } else { chars });
    let pos = pos.min(endpos);
    match re.search_at(&text, pos, anchored, full, false).map_err(|e| re_error(i, &e))? {
        Some(caps) => Ok(Value::Native(Rc::new(MatchObj { re: re.clone(), pattern: pat.clone(), string: string.clone(), text, caps, pos, endpos }))),
        None => Ok(Value::None),
    }
}

/// All non-overlapping matches (Python 3.7+ rules for empty matches).
fn all_matches(i: &mut Interp, re: &Regex, text: &[char], count: usize) -> R<Vec<Vec<Option<usize>>>> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut must_advance = false;
    while pos <= text.len() {
        if count > 0 && out.len() >= count {
            break;
        }
        let Some(caps) = re.search_at(text, pos, false, false, must_advance).map_err(|e| re_error(i, &e))? else {
            break;
        };
        let (s, e) = (caps[0].unwrap(), caps[1].unwrap());
        must_advance = e == s;
        pos = e;
        out.push(caps);
        let _ = s;
    }
    Ok(out)
}

/// Expand a replacement template (`\1`, `\g<name>`, escapes).
fn expand_template(i: &mut Interp, re: &Regex, template: &str, text: &[char], caps: &[Option<usize>]) -> R<String> {
    let c: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut k = 0;
    let group = |g: usize, out: &mut String| {
        if let (Some(a), Some(b)) = (caps[2 * g], caps[2 * g + 1]) {
            out.extend(text[a..b].iter());
        }
    };
    while k < c.len() {
        if c[k] != '\\' {
            out.push(c[k]);
            k += 1;
            continue;
        }
        k += 1;
        let Some(&e) = c.get(k) else {
            return Err(re_error(i, "bad escape (end of pattern)"));
        };
        k += 1;
        match e {
            'g' => {
                if c.get(k) != Some(&'<') {
                    return Err(re_error(i, "missing <"));
                }
                let start = k + 1;
                let Some(end) = c[start..].iter().position(|&x| x == '>') else {
                    return Err(re_error(i, "missing >, unterminated name"));
                };
                let name: String = c[start..start + end].iter().collect();
                k = start + end + 1;
                let g = match name.parse::<usize>() {
                    Ok(n) => n,
                    Err(_) => match re.names.iter().find(|(n, _)| *n == name) {
                        Some((_, g)) => *g,
                        None => return Err(i.exc("IndexError", format!("unknown group name '{}'", name))),
                    },
                };
                if g > re.ngroups {
                    return Err(re_error(i, &format!("invalid group reference {}", g)));
                }
                group(g, &mut out);
            }
            '0' => {
                let mut v = 0u32;
                let mut n = 0;
                while n < 2 && matches!(c.get(k), Some(d) if ('0'..='7').contains(d)) {
                    v = v * 8 + c[k].to_digit(8).unwrap();
                    k += 1;
                    n += 1;
                }
                out.push(char::from_u32(v).unwrap());
            }
            '1'..='9' => {
                let mut digits = e.to_string();
                if matches!(c.get(k), Some(d) if d.is_ascii_digit()) {
                    let d2 = c[k];
                    if ('0'..='7').contains(&e) && ('0'..='7').contains(&d2) && matches!(c.get(k + 1), Some(d3) if ('0'..='7').contains(d3)) {
                        let oct: String = [e, d2, c[k + 1]].iter().collect();
                        k += 2;
                        out.push(char::from_u32(u32::from_str_radix(&oct, 8).unwrap()).unwrap_or('\u{fffd}'));
                        continue;
                    }
                    digits.push(d2);
                    k += 1;
                }
                let g: usize = digits.parse().unwrap();
                if g > re.ngroups {
                    return Err(re_error(i, &format!("invalid group reference {}", g)));
                }
                group(g, &mut out);
            }
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'f' => out.push('\x0c'),
            'v' => out.push('\x0b'),
            'a' => out.push('\x07'),
            'b' => out.push('\x08'),
            '\\' => out.push('\\'),
            c2 if c2.is_ascii_alphabetic() => return Err(re_error(i, &format!("bad escape \\{}", c2))),
            c2 => {
                out.push('\\');
                out.push(c2);
            }
        }
    }
    Ok(out)
}

fn subn(i: &mut Interp, re: &Rc<Regex>, pat: &Value, repl: &Value, string: &Value, count: usize) -> R<(String, usize)> {
    let s = str_of(i, string)?;
    // Fast path: nothing to replace.
    let text: Rc<Vec<char>> = Rc::new(s.chars().collect());
    let matches = all_matches(i, re, &text, count)?;
    if matches.is_empty() {
        return Ok((s.to_string(), 0));
    }
    let template = match repl.unwrap_base() {
        Value::Str(t) => Some(t),
        _ => None,
    };
    // A template without backslashes is a literal.
    let literal = template.as_ref().filter(|t| !t.contains('\\')).cloned();
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    let n = matches.len();
    for caps in matches {
        let (a, b) = (caps[0].unwrap(), caps[1].unwrap());
        out.extend(text[last..a].iter());
        match (&literal, &template) {
            (Some(l), _) => out.push_str(l),
            (None, Some(t)) => out.push_str(&expand_template(i, re, t, &text, &caps)?),
            (None, None) => {
                let m = Value::Native(Rc::new(MatchObj { re: re.clone(), pattern: pat.clone(), string: string.clone(), text: text.clone(), caps: caps.clone(), pos: 0, endpos: text.len() }));
                let r = i.call(repl, vec![m], Vec::new())?;
                match r.unwrap_base() {
                    Value::Str(x) => out.push_str(&x),
                    other => return i.type_error(format!("expected str instance, {} found", other.type_name())),
                }
            }
        }
        last = b;
    }
    out.extend(text[last..].iter());
    Ok((out, n))
}

fn split(i: &mut Interp, re: &Rc<Regex>, string: &Value, maxsplit: usize) -> R<Value> {
    let s = str_of(i, string)?;
    let text: Vec<char> = s.chars().collect();
    let matches = all_matches(i, re, &text, maxsplit)?;
    let mut out = Vec::new();
    let mut last = 0;
    for caps in matches {
        let (a, b) = (caps[0].unwrap(), caps[1].unwrap());
        out.push(Value::Str(text[last..a].iter().collect::<String>().into()));
        for g in 1..=re.ngroups {
            out.push(match (caps[2 * g], caps[2 * g + 1]) {
                (Some(x), Some(y)) => Value::Str(text[x..y].iter().collect::<String>().into()),
                _ => Value::None,
            });
        }
        last = b;
    }
    out.push(Value::Str(text[last..].iter().collect::<String>().into()));
    Ok(Value::list(out))
}

fn findall(i: &mut Interp, re: &Rc<Regex>, string: &Value, pos: usize, endpos: Option<usize>) -> R<Value> {
    let s = str_of(i, string)?;
    let mut text: Vec<char> = s.chars().collect();
    if let Some(e) = endpos {
        text.truncate(e);
    }
    let sub: Vec<char> = text.clone();
    let matches = all_matches_from(i, re, &sub, pos.min(sub.len()))?;
    let g = |caps: &[Option<usize>], k: usize| -> Value {
        match (caps[2 * k], caps[2 * k + 1]) {
            (Some(a), Some(b)) => Value::Str(sub[a..b].iter().collect::<String>().into()),
            _ => Value::str(""),
        }
    };
    let out: Vec<Value> = matches
        .iter()
        .map(|caps| match re.ngroups {
            0 => g(caps, 0),
            1 => g(caps, 1),
            n => Value::tuple((1..=n).map(|k| g(caps, k)).collect()),
        })
        .collect();
    Ok(Value::list(out))
}

fn all_matches_from(i: &mut Interp, re: &Regex, text: &[char], start: usize) -> R<Vec<Vec<Option<usize>>>> {
    let mut out = Vec::new();
    let mut pos = start;
    let mut must_advance = false;
    while pos <= text.len() {
        let Some(caps) = re.search_at(text, pos, false, false, must_advance).map_err(|e| re_error(i, &e))? else {
            break;
        };
        let (s, e) = (caps[0].unwrap(), caps[1].unwrap());
        must_advance = e == s;
        pos = e;
        out.push(caps);
    }
    Ok(out)
}

fn finditer(i: &mut Interp, re: &Rc<Regex>, pat: &Value, string: &Value) -> R<Value> {
    let s = str_of(i, string)?;
    let text: Rc<Vec<char>> = Rc::new(s.chars().collect());
    let matches = all_matches(i, re, &text, 0)?;
    let items: Vec<Value> = matches.into_iter().map(|caps| Value::Native(Rc::new(MatchObj { re: re.clone(), pattern: pat.clone(), string: string.clone(), text: text.clone(), caps, pos: 0, endpos: text.len() }))).collect();
    Ok(Value::Iter(Rc::new(RefCell::new(PyIter::Seq(items, 0)))))
}

/// `re.escape`.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "()[]{}?*+-|^$\\.&~# \t\n\r\x0b\x0c".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl NativeObject for PatternObj {
    fn type_name(&self) -> &str {
        "re.Pattern"
    }
    fn call_method(&self, i: &mut Interp, this: &Value, name: &str, a: Vec<Value>, kw: Vec<(Name, Value)>) -> Option<R<Value>> {
        pattern_call(i, this, name, a, kw)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn repr(&self) -> String {
        format!("re.compile({})", str_repr(&self.re.pattern))
    }
    fn equals(&self, other: &Value) -> Option<bool> {
        if let Value::Native(n) = other {
            if let Some(o) = n.as_any().downcast_ref::<PatternObj>() {
                return Some(o.re.pattern == self.re.pattern && o.re.flags == self.re.flags);
            }
        }
        Some(false)
    }
    fn get_attr(&self, _i: &mut Interp, this: &Value, name: &str) -> Option<R<Value>> {
        let v = match name {
            "pattern" => Value::str(&self.re.pattern),
            "flags" => Value::Int((self.re.flags | U) as i64),
            "groups" => Value::Int(self.re.ngroups as i64),
            "groupindex" => {
                let mut d = Dict::default();
                for (n, k) in &self.re.names {
                    d.insert(HashKey::Str(n.as_str().into()), Value::str(n), Value::Int(*k as i64));
                }
                Value::dict(d)
            }
            "search" | "match" | "fullmatch" | "sub" | "subn" | "split" | "findall" | "finditer" => Value::Method(Rc::new((this.clone(), method_name(name)))),
            _ => return None,
        };
        Some(Ok(v))
    }
}

fn method_name(n: &str) -> &'static str {
    match n {
        "search" => "search",
        "match" => "match",
        "fullmatch" => "fullmatch",
        "sub" => "sub",
        "subn" => "subn",
        "split" => "split",
        "findall" => "findall",
        _ => "finditer",
    }
}

/// Methods of compiled patterns.
pub fn pattern_call(i: &mut Interp, pat: &Value, name: &str, a: Vec<Value>, kw: Vec<(Name, Value)>) -> Option<R<Value>> {
    let pat = pat.clone();
    let Value::Native(n) = &pat else { return None };
    let re = n.as_any().downcast_ref::<PatternObj>()?.re.clone();
    Some((|| -> R<Value> {
        let string = a.first().cloned().or_else(|| kw_or(&kw, "string")).unwrap_or(Value::None);
        let posarg = |i: &mut Interp, k: usize, name: &str| -> R<Option<usize>> {
            match a.get(k).cloned().or_else(|| kw_or(&kw, name)) {
                None | Some(Value::None) => Ok(None),
                Some(v) => Ok(Some(to_int(i, &v)?.max(0) as usize)),
            }
        };
        match name {
            "search" | "match" | "fullmatch" => {
                let pos = posarg(i, 1, "pos")?.unwrap_or(0);
                let endpos = posarg(i, 2, "endpos")?;
                do_search(i, &re, &pat, &string, pos, endpos, name != "search", name == "fullmatch")
            }
            "sub" | "subn" => {
                let repl = a.first().cloned().or_else(|| kw_or(&kw, "repl")).unwrap_or(Value::None);
                let string = a.get(1).cloned().or_else(|| kw_or(&kw, "string")).unwrap_or(Value::None);
                let count = match a.get(2).cloned().or_else(|| kw_or(&kw, "count")) {
                    Some(v) => to_int(i, &v)?.max(0) as usize,
                    None => 0,
                };
                let (s, n) = subn(i, &re, &pat, &repl, &string, count)?;
                Ok(if name == "sub" { Value::Str(s.into()) } else { Value::tuple(vec![Value::Str(s.into()), Value::Int(n as i64)]) })
            }
            "split" => {
                let maxsplit = posarg(i, 1, "maxsplit")?.unwrap_or(0);
                split(i, &re, &string, maxsplit)
            }
            "findall" => {
                let pos = posarg(i, 1, "pos")?.unwrap_or(0);
                let endpos = posarg(i, 2, "endpos")?;
                findall(i, &re, &string, pos, endpos)
            }
            _ => finditer(i, &re, &pat, &string),
        }
    })())
}

impl NativeObject for MatchObj {
    fn type_name(&self) -> &str {
        "re.Match"
    }
    fn truthy(&self) -> bool {
        true
    }
    fn call_method(&self, i: &mut Interp, this: &Value, name: &str, a: Vec<Value>, kw: Vec<(Name, Value)>) -> Option<R<Value>> {
        match_call(i, this, name, a, kw)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn repr(&self) -> String {
        let (a, b) = self.span(0);
        let m: String = self.text[a as usize..b as usize].iter().collect();
        format!("<re.Match object; span=({}, {}), match={}>", a, b, str_repr(&m))
    }
    fn get_item(&self, i: &mut Interp, k: &Value) -> Option<R<Value>> {
        Some(self.group_index(i, k).map(|g| self.group_str(g)))
    }
    fn get_attr(&self, _i: &mut Interp, this: &Value, name: &str) -> Option<R<Value>> {
        let v = match name {
            "string" => self.string.clone(),
            "re" => self.pattern.clone(),
            "pos" => Value::Int(self.pos as i64),
            "endpos" => Value::Int(self.endpos as i64),
            "lastindex" => {
                // The last group that closed (highest end position, last index on ties).
                let mut best: Option<(usize, usize)> = None;
                for g in 1..=self.re.ngroups {
                    if let (Some(_), Some(e)) = (self.caps[2 * g], self.caps[2 * g + 1]) {
                        if best.is_none_or(|(be, _)| e >= be) {
                            best = Some((e, g));
                        }
                    }
                }
                best.map(|(_, g)| Value::Int(g as i64)).unwrap_or(Value::None)
            }
            "lastgroup" => Value::None,
            "group" | "groups" | "groupdict" | "start" | "end" | "span" | "expand" => {
                let nm: &'static str = match name {
                    "group" => "group",
                    "groups" => "groups",
                    "groupdict" => "groupdict",
                    "start" => "start",
                    "end" => "end",
                    "span" => "span",
                    _ => "expand",
                };
                Value::Method(Rc::new((this.clone(), nm)))
            }
            _ => return None,
        };
        Some(Ok(v))
    }
}

pub fn match_call(i: &mut Interp, mv: &Value, name: &str, a: Vec<Value>, kw: Vec<(Name, Value)>) -> Option<R<Value>> {
    let Value::Native(n) = mv else { return None };
    let m = n.as_any().downcast_ref::<MatchObj>()?;
    Some((|| -> R<Value> {
        let gi = |i: &mut Interp, k: usize| -> R<usize> {
            match a.get(k) {
                Some(v) => m.group_index(i, v),
                None => Ok(0),
            }
        };
        match name {
            "group" => {
                if a.len() <= 1 {
                    let g = gi(i, 0)?;
                    Ok(m.group_str(g))
                } else {
                    let mut out = Vec::new();
                    for k in 0..a.len() {
                        let g = gi(i, k)?;
                        out.push(m.group_str(g));
                    }
                    Ok(Value::tuple(out))
                }
            }
            "groups" => {
                let default = a.first().cloned().or_else(|| kw_or(&kw, "default")).unwrap_or(Value::None);
                Ok(Value::tuple((1..=m.re.ngroups).map(|g| match m.group_str(g) {
                    Value::None => default.clone(),
                    v => v,
                }).collect()))
            }
            "groupdict" => {
                let default = a.first().cloned().or_else(|| kw_or(&kw, "default")).unwrap_or(Value::None);
                let mut d = Dict::default();
                for (name, g) in &m.re.names {
                    let v = match m.group_str(*g) {
                        Value::None => default.clone(),
                        v => v,
                    };
                    d.insert(HashKey::Str(name.as_str().into()), Value::str(name), v);
                }
                Ok(Value::dict(d))
            }
            "start" => Ok(Value::Int(m.span(gi(i, 0)?).0)),
            "end" => Ok(Value::Int(m.span(gi(i, 0)?).1)),
            "span" => {
                let (x, y) = m.span(gi(i, 0)?);
                Ok(Value::tuple(vec![Value::Int(x), Value::Int(y)]))
            }
            _ => {
                let t = str_of(i, a.first().unwrap_or(&Value::None))?;
                Ok(Value::Str(expand_template(i, &m.re, &t, &m.text, &m.caps)?.into()))
            }
        }
    })())
}

// ---------------------------------------------------------------------- module functions

fn re_compile(i: &mut Interp, a: Vec<Value>, kw: Vec<(Name, Value)>) -> R<Value> {
    let flags = flags_arg(i, a.get(1).cloned().or_else(|| kw_or(&kw, "flags")))?;
    let (_, p) = pattern_of(i, a.first().unwrap_or(&Value::None), flags)?;
    Ok(p)
}

fn re_func(i: &mut Interp, a: Vec<Value>, kw: Vec<(Name, Value)>, which: &'static str) -> R<Value> {
    let pat_v = a.first().cloned().or_else(|| kw_or(&kw, "pattern")).unwrap_or(Value::None);
    // Positional layout: (pattern, string, flags) or (pattern, repl, string, count, flags).
    let (flag_pos, rest): (usize, Vec<Value>) = match which {
        "sub" | "subn" => (4, a.iter().skip(1).take(3).cloned().collect()),
        "split" => (3, a.iter().skip(1).take(2).cloned().collect()),
        _ => (2, a.iter().skip(1).take(1).cloned().collect()),
    };
    let flags = flags_arg(i, a.get(flag_pos).cloned().or_else(|| kw_or(&kw, "flags")))?;
    let (_re, pat) = pattern_of(i, &pat_v, flags)?;
    let kw2: Vec<(Name, Value)> = kw.into_iter().filter(|(k, _)| &**k != "flags" && &**k != "pattern").collect();
    pattern_call(i, &pat, which, rest, kw2).unwrap()
}

pub fn module() -> Value {
    let f = |name: &'static str, f: BuiltinFn| (name, builtin(name, f));
    make_module(
        "re",
        vec![
            f("compile", re_compile),
            f("search", |i, a, kw| re_func(i, a, kw, "search")),
            f("match", |i, a, kw| re_func(i, a, kw, "match")),
            f("fullmatch", |i, a, kw| re_func(i, a, kw, "fullmatch")),
            f("sub", |i, a, kw| re_func(i, a, kw, "sub")),
            f("subn", |i, a, kw| re_func(i, a, kw, "subn")),
            f("split", |i, a, kw| re_func(i, a, kw, "split")),
            f("findall", |i, a, kw| re_func(i, a, kw, "findall")),
            f("finditer", |i, a, kw| re_func(i, a, kw, "finditer")),
            f("escape", |i, a, _kw| {
                let s = str_of(i, a.first().unwrap_or(&Value::None))?;
                Ok(Value::Str(escape(&s).into()))
            }),
            f("purge", |_i, _a, _kw| {
                CACHE.with(|c| c.borrow_mut().clear());
                Ok(Value::None)
            }),
            ("I", Value::Int(I as i64)),
            ("IGNORECASE", Value::Int(I as i64)),
            ("L", Value::Int(L as i64)),
            ("LOCALE", Value::Int(L as i64)),
            ("M", Value::Int(M as i64)),
            ("MULTILINE", Value::Int(M as i64)),
            ("S", Value::Int(S as i64)),
            ("DOTALL", Value::Int(S as i64)),
            ("U", Value::Int(U as i64)),
            ("UNICODE", Value::Int(U as i64)),
            ("X", Value::Int(X as i64)),
            ("VERBOSE", Value::Int(X as i64)),
            ("A", Value::Int(A as i64)),
            ("ASCII", Value::Int(A as i64)),
            ("NOFLAG", Value::Int(0)),
        ],
    )
}

/// Install `re.error` once the module is imported.
pub fn error_class(i: &mut Interp) -> Value {
    if let Flow::Exc(Value::Instance(e)) = re_error(i, "") {
        return Value::Class(e.class.clone());
    }
    Value::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(p: &str, s: &str) -> Option<(usize, usize)> {
        let re = Regex::new(p, 0).unwrap();
        let t: Vec<char> = s.chars().collect();
        re.search_at(&t, 0, false, false, false).unwrap().map(|c| (c[0].unwrap(), c[1].unwrap()))
    }

    #[test]
    fn basics() {
        assert_eq!(find(r"^-?[0-9]+\.?[0-9]*", "-12.5 X"), Some((0, 5)));
        assert_eq!(find(r"M106(.*)\n", "G1\nM106 S255\nG0"), Some((3, 13)));
        assert_eq!(find(r";\[CAZD:DELETE:[\s\S]+?:CAZD\](\n|$)", "a;[CAZD:DELETE:x\ny:CAZD]\nb"), Some((1, 25)));
        assert_eq!(find(r"(?<!\.|\w|\d)0+(\d+)", "x 007"), Some((2, 5)));
        assert_eq!(find(r"(a|ab)(c|bcd)(d*)", "abcd"), Some((0, 4)));
        assert_eq!(find(r"\bfoo\b", "a foo b"), Some((2, 5)));
        assert_eq!(find(r"(?i)hello", "HeLLo"), Some((0, 5)));
        assert_eq!(find(r"x{2,3}?", "xxxx"), Some((0, 2)));
        assert_eq!(find(r"(\w)\1", "abccd"), Some((2, 4)));
        assert_eq!(find(r"a(?=b)", "acab"), Some((2, 3)));
        assert_eq!(find(r"(a*)*b", "aaab"), Some((0, 4)));
        assert_eq!(find(r"(?:a|)*c", "aac"), Some((0, 3)));
    }
}
