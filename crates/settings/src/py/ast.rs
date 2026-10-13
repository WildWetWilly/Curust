//! Expression AST and parser (Python 3.12 expression grammar, minus the parts setting formulas
//! never use: f-strings, bytes, complex numbers, `await`, walrus).

use super::lexer::{tokenize, Tok};
use super::value::Value;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    MatMul,
    LShift,
    RShift,
    BitOr,
    BitXor,
    BitAnd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Pos,
    Invert,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    NotIn,
    Is,
    IsNot,
}

#[derive(Clone, Debug)]
pub struct Comprehension {
    pub target: Target,
    pub iter: Expr,
    pub ifs: Vec<Expr>,
}

#[derive(Clone, Debug)]
pub enum Target {
    Name(Rc<str>),
    Tuple(Vec<Target>),
}

#[derive(Clone, Debug)]
pub enum Expr {
    Const(Value),
    Name(Rc<str>),
    List(Vec<Expr>),
    Tuple(Vec<Expr>),
    Set(Vec<Expr>),
    Dict(Vec<(Expr, Expr)>),
    BinOp(Box<Expr>, BinOp, Box<Expr>),
    UnaryOp(UnOp, Box<Expr>),
    /// `and` (true) / `or` (false) chains.
    BoolOp(bool, Vec<Expr>),
    Compare(Box<Expr>, Vec<(CmpOp, Expr)>),
    IfExp(Box<Expr>, Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>, Vec<(Rc<str>, Expr)>),
    Starred(Box<Expr>),
    Attribute(Box<Expr>, Rc<str>),
    Subscript(Box<Expr>, Box<Expr>),
    Slice(Option<Box<Expr>>, Option<Box<Expr>>, Option<Box<Expr>>),
    GeneratorExp(Box<Expr>, Vec<Comprehension>),
    ListComp(Box<Expr>, Vec<Comprehension>),
    SetComp(Box<Expr>, Vec<Comprehension>),
    DictComp(Box<Expr>, Box<Expr>, Vec<Comprehension>),
    Lambda(Vec<Rc<str>>, Box<Expr>),
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

type PResult<T> = Result<T, String>;

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos]
    }
    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }
    fn is_op(&self, op: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }
    fn eat(&mut self, op: &str) -> bool {
        if self.is_op(op) {
            self.next();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, op: &str) -> PResult<()> {
        if self.eat(op) {
            Ok(())
        } else {
            Err(format!("expected '{}'", op))
        }
    }

    /// `expression`: lambda or ternary.
    fn expression(&mut self) -> PResult<Expr> {
        if self.eat("lambda") {
            let mut params = Vec::new();
            while !self.is_op(":") {
                match self.next() {
                    Tok::Name(n) => params.push(n),
                    _ => return Err("invalid lambda".into()),
                }
                if !self.eat(",") {
                    break;
                }
            }
            self.expect(":")?;
            let body = self.expression()?;
            return Ok(Expr::Lambda(params, Box::new(body)));
        }
        let body = self.disjunction()?;
        if self.eat("if") {
            let test = self.disjunction()?;
            self.expect("else")?;
            let orelse = self.expression()?;
            return Ok(Expr::IfExp(Box::new(test), Box::new(body), Box::new(orelse)));
        }
        Ok(body)
    }

    fn disjunction(&mut self) -> PResult<Expr> {
        let first = self.conjunction()?;
        if !self.is_op("or") {
            return Ok(first);
        }
        let mut v = vec![first];
        while self.eat("or") {
            v.push(self.conjunction()?);
        }
        Ok(Expr::BoolOp(false, v))
    }

    fn conjunction(&mut self) -> PResult<Expr> {
        let first = self.inversion()?;
        if !self.is_op("and") {
            return Ok(first);
        }
        let mut v = vec![first];
        while self.eat("and") {
            v.push(self.inversion()?);
        }
        Ok(Expr::BoolOp(true, v))
    }

    fn inversion(&mut self) -> PResult<Expr> {
        if self.eat("not") {
            let e = self.inversion()?;
            return Ok(Expr::UnaryOp(UnOp::Not, Box::new(e)));
        }
        self.comparison()
    }

    fn comparison(&mut self) -> PResult<Expr> {
        let left = self.bitor()?;
        let mut ops = Vec::new();
        loop {
            let op = match self.peek() {
                Tok::Op("==") => CmpOp::Eq,
                Tok::Op("!=") => CmpOp::Ne,
                Tok::Op("<") => CmpOp::Lt,
                Tok::Op("<=") => CmpOp::Le,
                Tok::Op(">") => CmpOp::Gt,
                Tok::Op(">=") => CmpOp::Ge,
                Tok::Op("in") => CmpOp::In,
                Tok::Op("not") => {
                    // `not in`
                    if matches!(self.toks.get(self.pos + 1), Some(Tok::Op("in"))) {
                        self.next();
                        CmpOp::NotIn
                    } else {
                        break;
                    }
                }
                Tok::Op("is") => {
                    if matches!(self.toks.get(self.pos + 1), Some(Tok::Op("not"))) {
                        self.next();
                        CmpOp::IsNot
                    } else {
                        CmpOp::Is
                    }
                }
                _ => break,
            };
            self.next();
            ops.push((op, self.bitor()?));
        }
        if ops.is_empty() {
            Ok(left)
        } else {
            Ok(Expr::Compare(Box::new(left), ops))
        }
    }

    fn bitor(&mut self) -> PResult<Expr> {
        let mut e = self.bitxor()?;
        while self.eat("|") {
            e = Expr::BinOp(Box::new(e), BinOp::BitOr, Box::new(self.bitxor()?));
        }
        Ok(e)
    }
    fn bitxor(&mut self) -> PResult<Expr> {
        let mut e = self.bitand()?;
        while self.eat("^") {
            e = Expr::BinOp(Box::new(e), BinOp::BitXor, Box::new(self.bitand()?));
        }
        Ok(e)
    }
    fn bitand(&mut self) -> PResult<Expr> {
        let mut e = self.shift()?;
        while self.eat("&") {
            e = Expr::BinOp(Box::new(e), BinOp::BitAnd, Box::new(self.shift()?));
        }
        Ok(e)
    }
    fn shift(&mut self) -> PResult<Expr> {
        let mut e = self.sum()?;
        loop {
            let op = if self.eat("<<") {
                BinOp::LShift
            } else if self.eat(">>") {
                BinOp::RShift
            } else {
                break;
            };
            e = Expr::BinOp(Box::new(e), op, Box::new(self.sum()?));
        }
        Ok(e)
    }
    fn sum(&mut self) -> PResult<Expr> {
        let mut e = self.term()?;
        loop {
            let op = if self.eat("+") {
                BinOp::Add
            } else if self.eat("-") {
                BinOp::Sub
            } else {
                break;
            };
            e = Expr::BinOp(Box::new(e), op, Box::new(self.term()?));
        }
        Ok(e)
    }
    fn term(&mut self) -> PResult<Expr> {
        let mut e = self.factor()?;
        loop {
            let op = if self.eat("*") {
                BinOp::Mul
            } else if self.eat("/") {
                BinOp::Div
            } else if self.eat("//") {
                BinOp::FloorDiv
            } else if self.eat("%") {
                BinOp::Mod
            } else if self.eat("@") {
                BinOp::MatMul
            } else {
                break;
            };
            e = Expr::BinOp(Box::new(e), op, Box::new(self.factor()?));
        }
        Ok(e)
    }
    fn factor(&mut self) -> PResult<Expr> {
        if self.eat("-") {
            let e = self.factor()?;
            // Constant folding of negative literals (as CPython's AST optimizer would make them constants
            // only at compile time; evaluation is identical).
            return Ok(Expr::UnaryOp(UnOp::Neg, Box::new(e)));
        }
        if self.eat("+") {
            return Ok(Expr::UnaryOp(UnOp::Pos, Box::new(self.factor()?)));
        }
        if self.eat("~") {
            return Ok(Expr::UnaryOp(UnOp::Invert, Box::new(self.factor()?)));
        }
        self.power()
    }
    fn power(&mut self) -> PResult<Expr> {
        let base = self.primary()?;
        if self.eat("**") {
            let exp = self.factor()?;
            return Ok(Expr::BinOp(Box::new(base), BinOp::Pow, Box::new(exp)));
        }
        Ok(base)
    }

    fn primary(&mut self) -> PResult<Expr> {
        let mut e = self.atom()?;
        loop {
            if self.eat("(") {
                let (args, kwargs) = self.call_args()?;
                e = Expr::Call(Box::new(e), args, kwargs);
            } else if self.eat("[") {
                let idx = self.subscript()?;
                self.expect("]")?;
                e = Expr::Subscript(Box::new(e), Box::new(idx));
            } else if self.eat(".") {
                match self.next() {
                    Tok::Name(n) => e = Expr::Attribute(Box::new(e), n),
                    _ => return Err("invalid attribute".into()),
                }
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn slice_part(&mut self) -> PResult<Option<Box<Expr>>> {
        if self.is_op(":") || self.is_op("]") || self.is_op(",") {
            Ok(None)
        } else {
            Ok(Some(Box::new(self.expression()?)))
        }
    }

    fn subscript_item(&mut self) -> PResult<Expr> {
        let lower = self.slice_part()?;
        if self.eat(":") {
            let upper = self.slice_part()?;
            let step = if self.eat(":") { self.slice_part()? } else { None };
            return Ok(Expr::Slice(lower, upper, step));
        }
        lower.map(|b| *b).ok_or_else(|| "invalid subscript".to_string())
    }

    fn subscript(&mut self) -> PResult<Expr> {
        let first = self.subscript_item()?;
        if !self.is_op(",") {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat(",") {
            if self.is_op("]") {
                break;
            }
            items.push(self.subscript_item()?);
        }
        Ok(Expr::Tuple(items))
    }

    fn call_args(&mut self) -> PResult<(Vec<Expr>, Vec<(Rc<str>, Expr)>)> {
        let mut args = Vec::new();
        let mut kwargs = Vec::new();
        while !self.is_op(")") {
            if self.eat("*") {
                args.push(Expr::Starred(Box::new(self.expression()?)));
            } else if self.is_op("**") {
                return Err("**kwargs not supported".into());
            } else if matches!(self.peek(), Tok::Name(_)) && matches!(self.toks.get(self.pos + 1), Some(Tok::Op("="))) {
                let name = match self.next() {
                    Tok::Name(n) => n,
                    _ => unreachable!(),
                };
                self.next();
                kwargs.push((name, self.expression()?));
            } else {
                if !kwargs.is_empty() {
                    return Err("positional argument follows keyword argument".into());
                }
                let e = self.expression()?;
                if self.is_op("for") {
                    let gens = self.comprehensions()?;
                    args.push(Expr::GeneratorExp(Box::new(e), gens));
                } else {
                    args.push(e);
                }
            }
            if !self.eat(",") {
                break;
            }
        }
        self.expect(")")?;
        Ok((args, kwargs))
    }

    fn target(&mut self) -> PResult<Target> {
        let mut items = Vec::new();
        let paren = self.eat("(");
        loop {
            match self.next() {
                Tok::Name(n) => items.push(Target::Name(n)),
                _ => return Err("invalid comprehension target".into()),
            }
            if !self.eat(",") || self.is_op("in") || self.is_op(")") {
                break;
            }
        }
        if paren {
            self.expect(")")?;
        }
        if items.len() == 1 && !paren {
            Ok(items.pop().unwrap())
        } else {
            Ok(Target::Tuple(items))
        }
    }

    fn comprehensions(&mut self) -> PResult<Vec<Comprehension>> {
        let mut gens = Vec::new();
        while self.eat("for") {
            let target = self.target()?;
            self.expect("in")?;
            let iter = self.disjunction()?;
            let mut ifs = Vec::new();
            while self.eat("if") {
                ifs.push(self.disjunction()?);
            }
            gens.push(Comprehension { target, iter, ifs });
        }
        Ok(gens)
    }

    fn atom(&mut self) -> PResult<Expr> {
        match self.next() {
            Tok::Name(n) => Ok(Expr::Name(n)),
            Tok::Int(v) => Ok(Expr::Const(Value::Int(v))),
            Tok::BigInt(s) => Ok(Expr::Const(Value::Float(s.parse::<f64>().unwrap_or(f64::INFINITY)))),
            Tok::Float(v) => Ok(Expr::Const(Value::Float(v))),
            Tok::Str(s) => {
                let mut acc = s.to_string();
                while let Tok::Str(more) = self.peek().clone() {
                    self.next();
                    acc.push_str(&more);
                }
                Ok(Expr::Const(Value::Str(acc.into())))
            }
            Tok::Op("None") => Ok(Expr::Const(Value::None)),
            Tok::Op("True") => Ok(Expr::Const(Value::Bool(true))),
            Tok::Op("False") => Ok(Expr::Const(Value::Bool(false))),
            Tok::Op("...") => Err("Ellipsis is not supported".into()),
            Tok::Op("(") => {
                if self.eat(")") {
                    return Ok(Expr::Tuple(Vec::new()));
                }
                let first = if self.eat("*") { Expr::Starred(Box::new(self.bitor()?)) } else { self.expression()? };
                if self.is_op("for") {
                    let gens = self.comprehensions()?;
                    self.expect(")")?;
                    return Ok(Expr::GeneratorExp(Box::new(first), gens));
                }
                if self.eat(")") {
                    if matches!(first, Expr::Starred(_)) {
                        return Err("cannot use starred expression here".into());
                    }
                    return Ok(first);
                }
                let mut items = vec![first];
                while self.eat(",") {
                    if self.is_op(")") {
                        break;
                    }
                    items.push(if self.eat("*") { Expr::Starred(Box::new(self.bitor()?)) } else { self.expression()? });
                }
                self.expect(")")?;
                Ok(Expr::Tuple(items))
            }
            Tok::Op("[") => {
                if self.eat("]") {
                    return Ok(Expr::List(Vec::new()));
                }
                let first = if self.eat("*") { Expr::Starred(Box::new(self.bitor()?)) } else { self.expression()? };
                if self.is_op("for") {
                    let gens = self.comprehensions()?;
                    self.expect("]")?;
                    return Ok(Expr::ListComp(Box::new(first), gens));
                }
                let mut items = vec![first];
                while self.eat(",") {
                    if self.is_op("]") {
                        break;
                    }
                    items.push(if self.eat("*") { Expr::Starred(Box::new(self.bitor()?)) } else { self.expression()? });
                }
                self.expect("]")?;
                Ok(Expr::List(items))
            }
            Tok::Op("{") => {
                if self.eat("}") {
                    return Ok(Expr::Dict(Vec::new()));
                }
                let first = self.expression()?;
                if self.eat(":") {
                    let v = self.expression()?;
                    if self.is_op("for") {
                        let gens = self.comprehensions()?;
                        self.expect("}")?;
                        return Ok(Expr::DictComp(Box::new(first), Box::new(v), gens));
                    }
                    let mut items = vec![(first, v)];
                    while self.eat(",") {
                        if self.is_op("}") {
                            break;
                        }
                        let k = self.expression()?;
                        self.expect(":")?;
                        items.push((k, self.expression()?));
                    }
                    self.expect("}")?;
                    return Ok(Expr::Dict(items));
                }
                if self.is_op("for") {
                    let gens = self.comprehensions()?;
                    self.expect("}")?;
                    return Ok(Expr::SetComp(Box::new(first), gens));
                }
                let mut items = vec![first];
                while self.eat(",") {
                    if self.is_op("}") {
                        break;
                    }
                    items.push(self.expression()?);
                }
                self.expect("}")?;
                Ok(Expr::Set(items))
            }
            t => Err(format!("invalid syntax at {:?}", t)),
        }
    }
}

/// Parse a complete expression (`compile(code, ..., "eval")`).
pub fn parse_expression(src: &str) -> Result<Expr, String> {
    let toks = tokenize(src)?;
    let mut p = Parser { toks, pos: 0 };
    let e = p.expression()?;
    // A bare tuple without parentheses: `1, 2`.
    let e = if p.is_op(",") {
        let mut items = vec![e];
        while p.eat(",") {
            if matches!(p.peek(), Tok::End) {
                break;
            }
            items.push(p.expression()?);
        }
        Expr::Tuple(items)
    } else {
        e
    };
    if !matches!(p.peek(), Tok::End) {
        return Err(format!("invalid syntax near {:?}", p.peek()));
    }
    Ok(e)
}
