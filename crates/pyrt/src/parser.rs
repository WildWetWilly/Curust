//! Recursive-descent parser for the Python 3 subset Cura's scripts use (which is most of the
//! language apart from generators, async and type statements).

use crate::ast::*;
use crate::lexer::{self, FPart, Tok, Token};
use std::rc::Rc;

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

type PResult<T> = Result<T, String>;

pub fn parse_module(src: &str) -> PResult<Vec<Stmt>> {
    let toks = lexer::tokenize(src)?;
    let mut p = Parser { toks, pos: 0 };
    let mut out = Vec::new();
    while !p.at_eof() {
        if p.eat_tok(&Tok::Newline) {
            continue;
        }
        out.extend(p.statement()?);
    }
    Ok(out)
}

pub fn parse_expression(src: &str) -> PResult<Expr> {
    let toks = lexer::tokenize(src.trim())?;
    let mut p = Parser { toks, pos: 0 };
    let e = p.testlist_star()?;
    p.eat_tok(&Tok::Newline);
    if !p.at_eof() {
        return Err(format!("unexpected {:?} in expression", p.peek()));
    }
    Ok(e)
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }
    fn peek_at(&self, k: usize) -> &Tok {
        &self.toks[(self.pos + k).min(self.toks.len() - 1)].tok
    }
    fn line(&self) -> usize {
        self.toks[self.pos].line
    }
    fn at_eof(&self) -> bool {
        matches!(self.peek(), Tok::Eof)
    }
    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }
    fn is_op(&self, op: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }
    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Name(n) if n == kw)
    }
    fn eat_op(&mut self, op: &str) -> bool {
        if self.is_op(op) {
            self.next();
            true
        } else {
            false
        }
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.next();
            true
        } else {
            false
        }
    }
    fn eat_tok(&mut self, t: &Tok) -> bool {
        if self.peek() == t {
            self.next();
            true
        } else {
            false
        }
    }
    fn err<T>(&self, msg: &str) -> PResult<T> {
        Err(format!("line {}: {} (at {:?})", self.line(), msg, self.peek()))
    }
    fn expect_op(&mut self, op: &str) -> PResult<()> {
        if self.eat_op(op) {
            Ok(())
        } else {
            self.err(&format!("expected '{}'", op))
        }
    }
    fn expect_kw(&mut self, kw: &str) -> PResult<()> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            self.err(&format!("expected '{}'", kw))
        }
    }
    fn name(&mut self) -> PResult<Name> {
        match self.next() {
            Tok::Name(n) => Ok(n.into()),
            t => Err(format!("line {}: expected a name, got {:?}", self.line(), t)),
        }
    }

    // ------------------------------------------------------------------ statements

    fn block(&mut self) -> PResult<Vec<Stmt>> {
        if self.eat_tok(&Tok::Newline) {
            if !self.eat_tok(&Tok::Indent) {
                return self.err("expected an indented block");
            }
            let mut out = Vec::new();
            while !self.eat_tok(&Tok::Dedent) {
                if self.at_eof() {
                    break;
                }
                if self.eat_tok(&Tok::Newline) {
                    continue;
                }
                out.extend(self.statement()?);
            }
            Ok(out)
        } else {
            // Simple statements on the same line.
            self.simple_statements()
        }
    }

    fn statement(&mut self) -> PResult<Vec<Stmt>> {
        let line = self.line();
        let kw = match self.peek() {
            Tok::Name(n) => n.clone(),
            Tok::Op("@") => "@".into(),
            _ => String::new(),
        };
        let kind = match kw.as_str() {
            "if" => {
                self.next();
                let mut branches = Vec::new();
                let test = self.namedexpr_test()?;
                self.expect_op(":")?;
                branches.push((test, self.block()?));
                let mut orelse = Vec::new();
                loop {
                    if self.eat_kw("elif") {
                        let t = self.namedexpr_test()?;
                        self.expect_op(":")?;
                        branches.push((t, self.block()?));
                    } else if self.eat_kw("else") {
                        self.expect_op(":")?;
                        orelse = self.block()?;
                        break;
                    } else {
                        break;
                    }
                }
                StmtKind::If(branches, orelse)
            }
            "while" => {
                self.next();
                let test = self.namedexpr_test()?;
                self.expect_op(":")?;
                let body = self.block()?;
                let orelse = if self.eat_kw("else") {
                    self.expect_op(":")?;
                    self.block()?
                } else {
                    Vec::new()
                };
                StmtKind::While(test, body, orelse)
            }
            "for" => {
                self.next();
                let target = self.target_list()?;
                self.expect_kw("in")?;
                let iter = self.testlist_star()?;
                self.expect_op(":")?;
                let body = self.block()?;
                let orelse = if self.eat_kw("else") {
                    self.expect_op(":")?;
                    self.block()?
                } else {
                    Vec::new()
                };
                StmtKind::For(target, iter, body, orelse)
            }
            "try" => {
                self.next();
                self.expect_op(":")?;
                let body = self.block()?;
                let mut handlers = Vec::new();
                while self.eat_kw("except") {
                    let mut typ = None;
                    let mut name = None;
                    if !self.is_op(":") {
                        self.eat_op("*");
                        typ = Some(self.test()?);
                        if self.eat_kw("as") {
                            name = Some(self.name()?);
                        } else if self.eat_op(",") {
                            name = Some(self.name()?);
                        }
                    }
                    self.expect_op(":")?;
                    handlers.push(Handler { typ, name, body: self.block()? });
                }
                let orelse = if self.eat_kw("else") {
                    self.expect_op(":")?;
                    self.block()?
                } else {
                    Vec::new()
                };
                let finally = if self.eat_kw("finally") {
                    self.expect_op(":")?;
                    self.block()?
                } else {
                    Vec::new()
                };
                StmtKind::Try(body, handlers, orelse, finally)
            }
            "with" => {
                self.next();
                let mut items = Vec::new();
                let paren = self.is_op("(") && self.with_paren_items();
                if paren {
                    self.next();
                }
                loop {
                    let e = self.test()?;
                    let t = if self.eat_kw("as") { Some(self.target()?) } else { None };
                    items.push((e, t));
                    if !self.eat_op(",") {
                        break;
                    }
                    if paren && self.is_op(")") {
                        break;
                    }
                }
                if paren {
                    self.expect_op(")")?;
                }
                self.expect_op(":")?;
                StmtKind::With(items, self.block()?)
            }
            "def" => StmtKind::FuncDef(self.funcdef(Vec::new())?),
            "class" => StmtKind::ClassDef(self.classdef(Vec::new())?),
            "@" => {
                let mut decorators = Vec::new();
                while self.eat_op("@") {
                    decorators.push(self.namedexpr_test()?);
                    self.eat_tok(&Tok::Newline);
                }
                if self.is_kw("def") {
                    StmtKind::FuncDef(self.funcdef(decorators)?)
                } else if self.is_kw("class") {
                    StmtKind::ClassDef(self.classdef(decorators)?)
                } else {
                    return self.err("expected def or class after a decorator");
                }
            }
            "match" if self.is_match_statement() => {
                self.next();
                let subject = self.testlist_star()?;
                self.expect_op(":")?;
                self.eat_tok(&Tok::Newline);
                if !self.eat_tok(&Tok::Indent) {
                    return self.err("expected an indented block");
                }
                let mut cases = Vec::new();
                while !self.eat_tok(&Tok::Dedent) {
                    if self.eat_tok(&Tok::Newline) {
                        continue;
                    }
                    self.expect_kw("case")?;
                    let pattern = self.pattern_top()?;
                    let guard = if self.eat_kw("if") { Some(self.namedexpr_test()?) } else { None };
                    self.expect_op(":")?;
                    cases.push(MatchCase { pattern, guard, body: self.block()? });
                }
                StmtKind::Match(subject, cases)
            }
            _ => return self.simple_statements(),
        };
        Ok(vec![Stmt { line, kind }])
    }

    fn with_paren_items(&self) -> bool {
        // `with (a as b, c as d):` — look for `as` at depth 1 before the matching ')'.
        let mut depth = 0;
        let mut k = 0;
        loop {
            match self.peek_at(k) {
                Tok::Op("(") | Tok::Op("[") | Tok::Op("{") => depth += 1,
                Tok::Op(")") | Tok::Op("]") | Tok::Op("}") => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(self.peek_at(k + 1), Tok::Op(":"));
                    }
                }
                Tok::Name(n) if n == "as" && depth == 1 => return true,
                Tok::Eof | Tok::Newline => return false,
                _ => {}
            }
            k += 1;
        }
    }

    fn is_match_statement(&self) -> bool {
        // `match x:` (soft keyword) vs. an expression using a name `match`.
        if matches!(self.peek_at(1), Tok::Op("=") | Tok::Op(".") | Tok::Op("(") | Tok::Op("[") | Tok::Op(",") | Tok::Op(")") | Tok::Newline) && !matches!(self.peek_at(1), Tok::Op("(") | Tok::Op("[")) {
            return false;
        }
        let mut depth = 0;
        let mut k = 1;
        loop {
            match self.peek_at(k) {
                Tok::Op("(") | Tok::Op("[") | Tok::Op("{") => depth += 1,
                Tok::Op(")") | Tok::Op("]") | Tok::Op("}") => depth -= 1,
                Tok::Op(":") if depth == 0 => return matches!(self.peek_at(k + 1), Tok::Newline),
                Tok::Op("=") if depth == 0 => return false,
                Tok::Newline | Tok::Eof => return false,
                _ => {}
            }
            k += 1;
        }
    }

    fn pattern_top(&mut self) -> PResult<Pattern> {
        let first = self.pattern_or()?;
        if self.is_op(",") {
            let mut items = vec![first];
            while self.eat_op(",") {
                if self.is_op(":") || self.is_kw("if") {
                    break;
                }
                items.push(self.pattern_or()?);
            }
            return Ok(Pattern::Sequence(items));
        }
        Ok(first)
    }

    fn pattern_or(&mut self) -> PResult<Pattern> {
        let mut alts = vec![self.pattern_atom()?];
        while self.eat_op("|") {
            alts.push(self.pattern_atom()?);
        }
        let p = if alts.len() == 1 { alts.pop().unwrap() } else { Pattern::Or(alts) };
        if self.eat_kw("as") {
            let n = self.name()?;
            return Ok(Pattern::As(Box::new(p), n));
        }
        Ok(p)
    }

    fn pattern_atom(&mut self) -> PResult<Pattern> {
        match self.peek().clone() {
            Tok::Name(n) if n == "_" => {
                self.next();
                Ok(Pattern::Wildcard)
            }
            Tok::Name(n) if !matches!(n.as_str(), "None" | "True" | "False") && !matches!(self.peek_at(1), Tok::Op(".") | Tok::Op("(")) => {
                self.next();
                Ok(Pattern::Capture(n.into()))
            }
            Tok::Op("[") | Tok::Op("(") => {
                let close = if self.is_op("[") { "]" } else { ")" };
                self.next();
                let mut items = Vec::new();
                while !self.is_op(close) {
                    items.push(self.pattern_or()?);
                    if !self.eat_op(",") {
                        break;
                    }
                }
                self.expect_op(close)?;
                if close == ")" && items.len() == 1 {
                    return Ok(items.pop().unwrap());
                }
                Ok(Pattern::Sequence(items))
            }
            _ => Ok(Pattern::Value(self.arith()?)),
        }
    }

    fn funcdef(&mut self, decorators: Vec<Expr>) -> PResult<Rc<FuncDef>> {
        self.expect_kw("def")?;
        let name = self.name()?;
        self.expect_op("(")?;
        let params = self.params(")")?;
        self.expect_op(")")?;
        if self.eat_op("->") {
            self.test()?;
        }
        self.expect_op(":")?;
        let body = self.block()?;
        Ok(Rc::new(FuncDef { name, params, body, decorators, is_lambda: false }))
    }

    fn params(&mut self, end: &str) -> PResult<Params> {
        let mut p = Params::default();
        let mut kwonly = false;
        while !self.is_op(end) {
            if self.eat_op("/") {
                // positional-only marker
            } else if self.eat_op("**") {
                p.kwarg = Some(self.name()?);
                if end == ")" && self.eat_op(":") {
                    self.test()?;
                }
            } else if self.eat_op("*") {
                kwonly = true;
                if !self.is_op(",") && !self.is_op(end) {
                    p.vararg = Some(self.name()?);
                    if end == ")" && self.eat_op(":") {
                        self.test()?;
                    }
                }
            } else {
                let n = self.name()?;
                if end == ")" && self.eat_op(":") {
                    self.test()?;
                }
                let default = if self.eat_op("=") { Some(self.test()?) } else { None };
                if kwonly {
                    p.kwonly.push((n, default));
                } else {
                    p.args.push((n, default));
                }
            }
            if !self.eat_op(",") {
                break;
            }
        }
        Ok(p)
    }

    fn classdef(&mut self, decorators: Vec<Expr>) -> PResult<Rc<ClassDef>> {
        self.expect_kw("class")?;
        let name = self.name()?;
        let mut bases = Vec::new();
        let mut keywords = Vec::new();
        if self.eat_op("(") {
            while !self.is_op(")") {
                if matches!(self.peek(), Tok::Name(_)) && matches!(self.peek_at(1), Tok::Op("=")) {
                    let k = self.name()?;
                    self.next();
                    keywords.push((k, self.test()?));
                } else {
                    bases.push(self.test()?);
                }
                if !self.eat_op(",") {
                    break;
                }
            }
            self.expect_op(")")?;
        }
        self.expect_op(":")?;
        let body = self.block()?;
        Ok(Rc::new(ClassDef { name, bases, keywords, body, decorators }))
    }

    fn simple_statements(&mut self) -> PResult<Vec<Stmt>> {
        let mut out = Vec::new();
        loop {
            let line = self.line();
            let kind = self.small_statement()?;
            out.push(Stmt { line, kind });
            if !self.eat_op(";") {
                break;
            }
            if matches!(self.peek(), Tok::Newline | Tok::Eof) {
                break;
            }
        }
        if !self.eat_tok(&Tok::Newline) && !self.at_eof() && !matches!(self.peek(), Tok::Dedent) {
            return self.err("expected end of statement");
        }
        Ok(out)
    }

    fn dotted_name(&mut self) -> PResult<Vec<Name>> {
        let mut v = vec![self.name()?];
        while self.eat_op(".") {
            v.push(self.name()?);
        }
        Ok(v)
    }

    fn small_statement(&mut self) -> PResult<StmtKind> {
        let kw = match self.peek() {
            Tok::Name(n) => n.clone(),
            _ => String::new(),
        };
        Ok(match kw.as_str() {
            "pass" => {
                self.next();
                StmtKind::Pass
            }
            "break" => {
                self.next();
                StmtKind::Break
            }
            "continue" => {
                self.next();
                StmtKind::Continue
            }
            "return" => {
                self.next();
                if matches!(self.peek(), Tok::Newline | Tok::Eof | Tok::Dedent) || self.is_op(";") {
                    StmtKind::Return(None)
                } else {
                    StmtKind::Return(Some(self.testlist_star()?))
                }
            }
            "raise" => {
                self.next();
                if matches!(self.peek(), Tok::Newline | Tok::Eof) || self.is_op(";") {
                    StmtKind::Raise(None, None)
                } else {
                    let e = self.test()?;
                    let cause = if self.eat_kw("from") { Some(self.test()?) } else { None };
                    StmtKind::Raise(Some(e), cause)
                }
            }
            "global" | "nonlocal" => {
                self.next();
                let mut names = vec![self.name()?];
                while self.eat_op(",") {
                    names.push(self.name()?);
                }
                if kw == "global" {
                    StmtKind::Global(names)
                } else {
                    StmtKind::Nonlocal(names)
                }
            }
            "del" => {
                self.next();
                let mut targets = vec![self.target()?];
                while self.eat_op(",") {
                    if matches!(self.peek(), Tok::Newline) {
                        break;
                    }
                    targets.push(self.target()?);
                }
                StmtKind::Del(targets)
            }
            "assert" => {
                self.next();
                let t = self.test()?;
                let msg = if self.eat_op(",") { Some(self.test()?) } else { None };
                StmtKind::Assert(t, msg)
            }
            "import" => {
                self.next();
                let mut items = Vec::new();
                loop {
                    let path = self.dotted_name()?;
                    let alias = if self.eat_kw("as") { Some(self.name()?) } else { None };
                    items.push((path, alias));
                    if !self.eat_op(",") {
                        break;
                    }
                }
                StmtKind::Import(items)
            }
            "from" => {
                self.next();
                let mut level = 0;
                loop {
                    if self.eat_op(".") {
                        level += 1;
                    } else if self.eat_op("...") {
                        level += 3;
                    } else {
                        break;
                    }
                }
                let path = if self.is_kw("import") { Vec::new() } else { self.dotted_name()? };
                self.expect_kw("import")?;
                let mut names = Vec::new();
                if self.eat_op("*") {
                    names.push(("*".into(), None));
                } else {
                    let paren = self.eat_op("(");
                    loop {
                        if paren && self.is_op(")") {
                            break;
                        }
                        let n = self.name()?;
                        let alias = if self.eat_kw("as") { Some(self.name()?) } else { None };
                        names.push((n, alias));
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                    if paren {
                        self.expect_op(")")?;
                    }
                }
                StmtKind::ImportFrom(level, path, names)
            }
            _ => self.expr_statement()?,
        })
    }

    fn expr_statement(&mut self) -> PResult<StmtKind> {
        let first = self.testlist_star()?;
        // Annotated assignment.
        if self.is_op(":") {
            self.next();
            self.test()?;
            let value = if self.eat_op("=") { Some(self.testlist_star()?) } else { None };
            return Ok(StmtKind::AnnAssign(first, value));
        }
        let aug = match self.peek() {
            Tok::Op(o) => match *o {
                "+=" => Some(BinOp::Add),
                "-=" => Some(BinOp::Sub),
                "*=" => Some(BinOp::Mul),
                "/=" => Some(BinOp::Div),
                "//=" => Some(BinOp::FloorDiv),
                "%=" => Some(BinOp::Mod),
                "**=" => Some(BinOp::Pow),
                ">>=" => Some(BinOp::RShift),
                "<<=" => Some(BinOp::LShift),
                "&=" => Some(BinOp::BitAnd),
                "|=" => Some(BinOp::BitOr),
                "^=" => Some(BinOp::BitXor),
                "@=" => Some(BinOp::MatMul),
                _ => None,
            },
            _ => None,
        };
        if let Some(op) = aug {
            self.next();
            let value = self.testlist_star()?;
            return Ok(StmtKind::AugAssign(first, op, value));
        }
        if self.is_op("=") {
            let mut targets = vec![first];
            let mut value;
            loop {
                self.expect_op("=")?;
                value = self.testlist_star()?;
                if self.is_op("=") {
                    targets.push(value);
                } else {
                    break;
                }
            }
            return Ok(StmtKind::Assign(targets, value));
        }
        Ok(StmtKind::Expr(first))
    }

    // ------------------------------------------------------------------ expressions

    /// A target list for `for` (stops before `in`).
    fn target_list(&mut self) -> PResult<Expr> {
        let first = self.target()?;
        if self.is_op(",") {
            let mut items = vec![first];
            while self.eat_op(",") {
                if self.is_kw("in") || self.is_op("=") {
                    break;
                }
                items.push(self.target()?);
            }
            return Ok(Expr::Tuple(items));
        }
        Ok(first)
    }

    fn target(&mut self) -> PResult<Expr> {
        if self.eat_op("*") {
            return Ok(Expr::Starred(Box::new(self.target()?)));
        }
        self.bitor()
    }

    /// `testlist_star_expr`: a tuple when there are commas.
    pub fn testlist_star(&mut self) -> PResult<Expr> {
        let first = self.star_or_test()?;
        if self.is_op(",") {
            let mut items = vec![first];
            while self.eat_op(",") {
                if self.ends_list() {
                    break;
                }
                items.push(self.star_or_test()?);
            }
            return Ok(Expr::Tuple(items));
        }
        Ok(first)
    }

    fn ends_list(&self) -> bool {
        matches!(self.peek(), Tok::Newline | Tok::Eof | Tok::Op("=") | Tok::Op(")") | Tok::Op("]") | Tok::Op("}") | Tok::Op(":") | Tok::Op(";"))
            || matches!(self.peek(), Tok::Op(o) if o.ends_with('=') && *o != "==" && *o != "<=" && *o != ">=" && *o != "!=")
    }

    fn star_or_test(&mut self) -> PResult<Expr> {
        if self.eat_op("*") {
            return Ok(Expr::Starred(Box::new(self.bitor()?)));
        }
        self.namedexpr_test()
    }

    fn namedexpr_test(&mut self) -> PResult<Expr> {
        if let Tok::Name(n) = self.peek().clone() {
            if matches!(self.peek_at(1), Tok::Op(":=")) {
                self.next();
                self.next();
                let v = self.test()?;
                return Ok(Expr::NamedExpr(n.into(), Box::new(v)));
            }
        }
        self.test()
    }

    pub fn test(&mut self) -> PResult<Expr> {
        if self.is_kw("lambda") {
            return self.lambda();
        }
        let e = self.or_test()?;
        if self.is_kw("if") {
            // Not part of a comprehension clause: the caller handles `for ... if`.
            let save = self.pos;
            self.next();
            let cond = self.or_test()?;
            if self.eat_kw("else") {
                let other = self.test()?;
                return Ok(Expr::IfExp(Box::new(cond), Box::new(e), Box::new(other)));
            }
            self.pos = save;
        }
        Ok(e)
    }

    fn test_nocond(&mut self) -> PResult<Expr> {
        if self.is_kw("lambda") {
            return self.lambda();
        }
        self.or_test()
    }

    fn lambda(&mut self) -> PResult<Expr> {
        self.expect_kw("lambda")?;
        let params = self.params(":")?;
        self.expect_op(":")?;
        let line = self.line();
        let body = self.test()?;
        Ok(Expr::Lambda(Rc::new(FuncDef {
            name: "<lambda>".into(),
            params,
            body: vec![Stmt { line, kind: StmtKind::Return(Some(body)) }],
            decorators: Vec::new(),
            is_lambda: true,
        })))
    }

    fn or_test(&mut self) -> PResult<Expr> {
        let mut e = self.and_test()?;
        while self.eat_kw("or") {
            let r = self.and_test()?;
            e = Expr::Or(Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn and_test(&mut self) -> PResult<Expr> {
        let mut e = self.not_test()?;
        while self.eat_kw("and") {
            let r = self.not_test()?;
            e = Expr::And(Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn not_test(&mut self) -> PResult<Expr> {
        if self.eat_kw("not") {
            return Ok(Expr::Unary(UnOp::Not, Box::new(self.not_test()?)));
        }
        self.comparison()
    }

    fn comparison(&mut self) -> PResult<Expr> {
        let first = self.bitor()?;
        let mut ops = Vec::new();
        loop {
            let op = match self.peek() {
                Tok::Op("==") => CmpOp::Eq,
                Tok::Op("!=") => CmpOp::NotEq,
                Tok::Op("<") => CmpOp::Lt,
                Tok::Op("<=") => CmpOp::LtE,
                Tok::Op(">") => CmpOp::Gt,
                Tok::Op(">=") => CmpOp::GtE,
                Tok::Name(n) if n == "in" => CmpOp::In,
                Tok::Name(n) if n == "is" => {
                    if matches!(self.peek_at(1), Tok::Name(m) if m == "not") {
                        self.next();
                        CmpOp::IsNot
                    } else {
                        CmpOp::Is
                    }
                }
                Tok::Name(n) if n == "not" && matches!(self.peek_at(1), Tok::Name(m) if m == "in") => {
                    self.next();
                    CmpOp::NotIn
                }
                _ => break,
            };
            self.next();
            ops.push((op, self.bitor()?));
        }
        if ops.is_empty() {
            Ok(first)
        } else {
            Ok(Expr::Compare(Box::new(first), ops))
        }
    }

    fn binary_level(&mut self, level: usize) -> PResult<Expr> {
        const LEVELS: &[&[(&str, BinOp)]] = &[
            &[("|", BinOp::BitOr)],
            &[("^", BinOp::BitXor)],
            &[("&", BinOp::BitAnd)],
            &[("<<", BinOp::LShift), (">>", BinOp::RShift)],
            &[("+", BinOp::Add), ("-", BinOp::Sub)],
            &[("*", BinOp::Mul), ("/", BinOp::Div), ("//", BinOp::FloorDiv), ("%", BinOp::Mod), ("@", BinOp::MatMul)],
        ];
        if level >= LEVELS.len() {
            return self.factor();
        }
        let mut e = self.binary_level(level + 1)?;
        loop {
            let op = LEVELS[level].iter().find(|(s, _)| self.is_op(s)).map(|(_, o)| *o);
            match op {
                Some(o) => {
                    self.next();
                    let r = self.binary_level(level + 1)?;
                    e = Expr::BinOp(Box::new(e), o, Box::new(r));
                }
                None => break,
            }
        }
        Ok(e)
    }

    fn bitor(&mut self) -> PResult<Expr> {
        self.binary_level(0)
    }

    fn arith(&mut self) -> PResult<Expr> {
        self.binary_level(4)
    }

    fn factor(&mut self) -> PResult<Expr> {
        if self.eat_op("-") {
            return Ok(Expr::Unary(UnOp::Neg, Box::new(self.factor()?)));
        }
        if self.eat_op("+") {
            return Ok(Expr::Unary(UnOp::Pos, Box::new(self.factor()?)));
        }
        if self.eat_op("~") {
            return Ok(Expr::Unary(UnOp::Invert, Box::new(self.factor()?)));
        }
        self.power()
    }

    fn power(&mut self) -> PResult<Expr> {
        self.eat_kw("await");
        let base = self.primary()?;
        if self.eat_op("**") {
            let exp = self.factor()?;
            return Ok(Expr::BinOp(Box::new(base), BinOp::Pow, Box::new(exp)));
        }
        Ok(base)
    }

    fn primary(&mut self) -> PResult<Expr> {
        let mut e = self.atom()?;
        loop {
            if self.eat_op(".") {
                let n = self.name()?;
                e = Expr::Attr(Box::new(e), n);
            } else if self.is_op("(") {
                self.next();
                let args = self.call_args()?;
                self.expect_op(")")?;
                e = Expr::Call(Box::new(e), args);
            } else if self.is_op("[") {
                self.next();
                let idx = self.subscript_list()?;
                self.expect_op("]")?;
                e = Expr::Subscript(Box::new(e), Box::new(idx));
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn call_args(&mut self) -> PResult<Vec<Arg>> {
        let mut args = Vec::new();
        while !self.is_op(")") {
            if self.eat_op("**") {
                args.push(Arg::StarStar(self.test()?));
            } else if self.eat_op("*") {
                args.push(Arg::Star(self.test()?));
            } else if matches!(self.peek(), Tok::Name(_)) && matches!(self.peek_at(1), Tok::Op("=")) {
                let n = self.name()?;
                self.next();
                args.push(Arg::Kw(n, self.test()?));
            } else {
                let e = self.namedexpr_test()?;
                if self.is_kw("for") {
                    let comps = self.comp_for()?;
                    args.push(Arg::Pos(Expr::GenExp(Box::new(e), Rc::new(comps))));
                } else {
                    args.push(Arg::Pos(e));
                }
            }
            if !self.eat_op(",") {
                break;
            }
        }
        Ok(args)
    }

    fn subscript_list(&mut self) -> PResult<Expr> {
        let first = self.subscript()?;
        if self.is_op(",") {
            let mut items = vec![first];
            while self.eat_op(",") {
                if self.is_op("]") {
                    break;
                }
                items.push(self.subscript()?);
            }
            return Ok(Expr::Tuple(items));
        }
        Ok(first)
    }

    fn subscript(&mut self) -> PResult<Expr> {
        let lower = if self.is_op(":") { None } else { Some(Box::new(self.namedexpr_test()?)) };
        if !self.eat_op(":") {
            return Ok(*lower.unwrap());
        }
        let upper = if self.is_op(":") || self.is_op("]") || self.is_op(",") { None } else { Some(Box::new(self.test()?)) };
        let step = if self.eat_op(":") {
            if self.is_op("]") || self.is_op(",") {
                None
            } else {
                Some(Box::new(self.test()?))
            }
        } else {
            None
        };
        Ok(Expr::Slice(lower, upper, step))
    }

    fn comp_for(&mut self) -> PResult<Vec<Comprehension>> {
        let mut comps = Vec::new();
        while self.eat_kw("for") || (self.is_kw("async") && { self.next(); self.eat_kw("for") }) {
            let target = self.target_list()?;
            self.expect_kw("in")?;
            let iter = self.or_test()?;
            let mut ifs = Vec::new();
            while self.eat_kw("if") {
                ifs.push(self.test_nocond()?);
            }
            comps.push(Comprehension { target, iter, ifs });
        }
        Ok(comps)
    }

    fn fstring(&mut self, parts: Vec<FPart>) -> PResult<Vec<FPartE>> {
        let mut out = Vec::new();
        for p in parts {
            match p {
                FPart::Lit(s) => out.push(FPartE::Lit(s.into())),
                FPart::Expr { src, conv, spec, debug } => {
                    let expr = parse_expression(&src).map_err(|e| format!("line {}: f-string: {}", self.line(), e))?;
                    let spec = self.fstring(spec)?;
                    out.push(FPartE::Expr { expr: Box::new(expr), conv, spec, debug: debug.map(|d| d.into()) });
                }
            }
        }
        Ok(out)
    }

    fn atom(&mut self) -> PResult<Expr> {
        let t = self.next();
        Ok(match t {
            Tok::Name(n) => match n.as_str() {
                "None" => Expr::Const(Const::None),
                "True" => Expr::Const(Const::Bool(true)),
                "False" => Expr::Const(Const::Bool(false)),
                _ => Expr::Name(n.into()),
            },
            Tok::Int(v) => Expr::Const(Const::Int(v)),
            Tok::BigInt(s) => Expr::Const(Const::Float(s.parse().unwrap_or(f64::INFINITY))),
            Tok::Float(f) => Expr::Const(Const::Float(f)),
            Tok::Imag(_) => return self.err("complex numbers are not supported"),
            Tok::Op("...") => Expr::Const(Const::Ellipsis),
            Tok::Str(..) | Tok::FStr(..) => {
                // Adjacent string literals concatenate.
                let mut parts: Vec<FPartE> = Vec::new();
                let mut bytes: Option<Vec<u8>> = None;
                let mut has_f = false;
                let mut cur = t;
                loop {
                    match cur {
                        Tok::Str(s, true) => bytes.get_or_insert_with(Vec::new).extend(s.chars().map(|c| c as u32 as u8)),
                        Tok::Str(s, false) => parts.push(FPartE::Lit(s.into())),
                        Tok::FStr(p) => {
                            has_f = true;
                            parts.extend(self.fstring(p)?);
                        }
                        _ => unreachable!(),
                    }
                    if matches!(self.peek(), Tok::Str(..) | Tok::FStr(..)) {
                        cur = self.next();
                    } else {
                        break;
                    }
                }
                if let Some(b) = bytes {
                    return Ok(Expr::Const(Const::Bytes(Rc::new(b))));
                }
                if has_f {
                    Expr::FString(parts)
                } else {
                    let s: String = parts.iter().map(|p| if let FPartE::Lit(s) = p { s.to_string() } else { String::new() }).collect();
                    Expr::Const(Const::Str(s.into()))
                }
            }
            Tok::Op("(") => {
                if self.eat_op(")") {
                    return Ok(Expr::Tuple(Vec::new()));
                }
                let first = self.star_or_test()?;
                if self.is_kw("for") {
                    let comps = self.comp_for()?;
                    self.expect_op(")")?;
                    return Ok(Expr::GenExp(Box::new(first), Rc::new(comps)));
                }
                if self.eat_op(")") {
                    return Ok(first);
                }
                let mut items = vec![first];
                while self.eat_op(",") {
                    if self.is_op(")") {
                        break;
                    }
                    items.push(self.star_or_test()?);
                }
                self.expect_op(")")?;
                Expr::Tuple(items)
            }
            Tok::Op("[") => {
                if self.eat_op("]") {
                    return Ok(Expr::List(Vec::new()));
                }
                let first = self.star_or_test()?;
                if self.is_kw("for") {
                    let comps = self.comp_for()?;
                    self.expect_op("]")?;
                    return Ok(Expr::ListComp(Box::new(first), Rc::new(comps)));
                }
                let mut items = vec![first];
                while self.eat_op(",") {
                    if self.is_op("]") {
                        break;
                    }
                    items.push(self.star_or_test()?);
                }
                self.expect_op("]")?;
                Expr::List(items)
            }
            Tok::Op("{") => {
                if self.eat_op("}") {
                    return Ok(Expr::Dict(Vec::new()));
                }
                if self.eat_op("**") {
                    let first = self.bitor()?;
                    let mut items = vec![(None, first)];
                    self.dict_rest(&mut items)?;
                    return Ok(Expr::Dict(items));
                }
                let first = self.star_or_test()?;
                if self.eat_op(":") {
                    let v = self.test()?;
                    if self.is_kw("for") {
                        let comps = self.comp_for()?;
                        self.expect_op("}")?;
                        return Ok(Expr::DictComp(Box::new(first), Box::new(v), Rc::new(comps)));
                    }
                    let mut items = vec![(Some(first), v)];
                    self.dict_rest(&mut items)?;
                    return Ok(Expr::Dict(items));
                }
                if self.is_kw("for") {
                    let comps = self.comp_for()?;
                    self.expect_op("}")?;
                    return Ok(Expr::SetComp(Box::new(first), Rc::new(comps)));
                }
                let mut items = vec![first];
                while self.eat_op(",") {
                    if self.is_op("}") {
                        break;
                    }
                    items.push(self.star_or_test()?);
                }
                self.expect_op("}")?;
                Expr::Set(items)
            }
            other => return Err(format!("line {}: unexpected {:?}", self.line(), other)),
        })
    }

    fn dict_rest(&mut self, items: &mut Vec<(Option<Expr>, Expr)>) -> PResult<()> {
        while self.eat_op(",") {
            if self.is_op("}") {
                break;
            }
            if self.eat_op("**") {
                items.push((None, self.bitor()?));
                continue;
            }
            let k = self.test()?;
            self.expect_op(":")?;
            let v = self.test()?;
            items.push((Some(k), v));
        }
        self.expect_op("}")
    }
}
