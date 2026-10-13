//! Abstract syntax tree.

use std::rc::Rc;

pub type Name = Rc<str>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    LShift,
    RShift,
    BitOr,
    BitXor,
    BitAnd,
    MatMul,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp {
    Neg,
    Pos,
    Invert,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CmpOp {
    Eq,
    NotEq,
    Lt,
    LtE,
    Gt,
    GtE,
    Is,
    IsNot,
    In,
    NotIn,
}

#[derive(Debug, Clone)]
pub enum Const {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Bytes(Rc<Vec<u8>>),
    Ellipsis,
}

#[derive(Debug, Clone)]
pub enum FPartE {
    Lit(Rc<str>),
    Expr { expr: Box<Expr>, conv: Option<char>, spec: Vec<FPartE>, debug: Option<Rc<str>> },
}

#[derive(Debug, Clone)]
pub enum Arg {
    Pos(Expr),
    Star(Expr),
    Kw(Name, Expr),
    StarStar(Expr),
}

#[derive(Debug, Clone)]
pub struct Comprehension {
    pub target: Expr,
    pub iter: Expr,
    pub ifs: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Name(Name),
    Const(Const),
    FString(Vec<FPartE>),
    List(Vec<Expr>),
    Tuple(Vec<Expr>),
    Set(Vec<Expr>),
    /// `None` key: `**mapping`.
    Dict(Vec<(Option<Expr>, Expr)>),
    Starred(Box<Expr>),
    Attr(Box<Expr>, Name),
    Subscript(Box<Expr>, Box<Expr>),
    Slice(Option<Box<Expr>>, Option<Box<Expr>>, Option<Box<Expr>>),
    Call(Box<Expr>, Vec<Arg>),
    BinOp(Box<Expr>, BinOp, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Compare(Box<Expr>, Vec<(CmpOp, Expr)>),
    IfExp(Box<Expr>, Box<Expr>, Box<Expr>),
    Lambda(Rc<FuncDef>),
    ListComp(Box<Expr>, Rc<Vec<Comprehension>>),
    SetComp(Box<Expr>, Rc<Vec<Comprehension>>),
    DictComp(Box<Expr>, Box<Expr>, Rc<Vec<Comprehension>>),
    GenExp(Box<Expr>, Rc<Vec<Comprehension>>),
    NamedExpr(Name, Box<Expr>),
}

#[derive(Debug, Clone, Default)]
pub struct Params {
    pub args: Vec<(Name, Option<Expr>)>,
    pub vararg: Option<Name>,
    pub kwonly: Vec<(Name, Option<Expr>)>,
    pub kwarg: Option<Name>,
}

#[derive(Debug)]
pub struct FuncDef {
    pub name: Name,
    pub params: Params,
    pub body: Vec<Stmt>,
    pub decorators: Vec<Expr>,
    pub is_lambda: bool,
}

#[derive(Debug)]
pub struct ClassDef {
    pub name: Name,
    pub bases: Vec<Expr>,
    pub keywords: Vec<(Name, Expr)>,
    pub body: Vec<Stmt>,
    pub decorators: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub struct Handler {
    pub typ: Option<Expr>,
    pub name: Option<Name>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard,
    Capture(Name),
    Value(Expr),
    Or(Vec<Pattern>),
    Sequence(Vec<Pattern>),
    As(Box<Pattern>, Name),
}

#[derive(Debug, Clone)]
pub struct MatchCase {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    Expr(Expr),
    Assign(Vec<Expr>, Expr),
    AugAssign(Expr, BinOp, Expr),
    AnnAssign(Expr, Option<Expr>),
    If(Vec<(Expr, Vec<Stmt>)>, Vec<Stmt>),
    While(Expr, Vec<Stmt>, Vec<Stmt>),
    For(Expr, Expr, Vec<Stmt>, Vec<Stmt>),
    Break,
    Continue,
    Pass,
    Return(Option<Expr>),
    Raise(Option<Expr>, Option<Expr>),
    Try(Vec<Stmt>, Vec<Handler>, Vec<Stmt>, Vec<Stmt>),
    With(Vec<(Expr, Option<Expr>)>, Vec<Stmt>),
    FuncDef(Rc<FuncDef>),
    ClassDef(Rc<ClassDef>),
    /// (dotted module, alias)
    Import(Vec<(Vec<Name>, Option<Name>)>),
    /// (level, module path, names (name, alias); `*` = all)
    ImportFrom(usize, Vec<Name>, Vec<(Name, Option<Name>)>),
    Global(Vec<Name>),
    Nonlocal(Vec<Name>),
    Del(Vec<Expr>),
    Assert(Expr, Option<Expr>),
    Match(Expr, Vec<MatchCase>),
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub line: usize,
    pub kind: StmtKind,
}
