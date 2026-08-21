//! Abstract syntax tree for XPath 1.0 expressions (production numbers as in
//! the [XPath 1.0 grammar](https://www.w3.org/TR/1999/REC-xpath-19991116/#exprlex),
//! see `plan/02-lexer-parser.md`).
//!
//! Abbreviated syntax (`@name`, `.`, `..`, `//`) is desugared during parsing
//! into its equivalent explicit-axis form — there is no separate
//! "abbreviated step" representation here, only `Step { axis, .. }` with the
//! axis the abbreviation stands for.

/// A qualified name (`NCName (':' NCName)?`, XML Namespaces §3).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QName {
    pub prefix: Option<String>,
    pub local: String,
}

/// The 13 XPath axes ([6] `AxisName`). Abbreviated forms (`@`, `.`, `..`)
/// are represented as their equivalent explicit axis, not as a separate
/// variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Ancestor,
    AncestorOrSelf,
    Attribute,
    Child,
    Descendant,
    DescendantOrSelf,
    Following,
    FollowingSibling,
    Namespace,
    Parent,
    Preceding,
    PrecedingSibling,
    SelfAxis,
}

impl Axis {
    /// Maps an `AxisName` token text to its `Axis`, or `None` if `name` is
    /// not one of the 13 known axis names.
    pub fn from_name(name: &str) -> Option<Axis> {
        Some(match name {
            "ancestor" => Axis::Ancestor,
            "ancestor-or-self" => Axis::AncestorOrSelf,
            "attribute" => Axis::Attribute,
            "child" => Axis::Child,
            "descendant" => Axis::Descendant,
            "descendant-or-self" => Axis::DescendantOrSelf,
            "following" => Axis::Following,
            "following-sibling" => Axis::FollowingSibling,
            "namespace" => Axis::Namespace,
            "parent" => Axis::Parent,
            "preceding" => Axis::Preceding,
            "preceding-sibling" => Axis::PrecedingSibling,
            "self" => Axis::SelfAxis,
            _ => return None,
        })
    }
}

/// [7] `NodeTest` — includes the `NameTest` alternatives (`*`, `prefix:*`,
/// `QName`) and the `NodeType`/`processing-instruction(Literal)` forms.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeTest {
    /// `*`
    AnyName,
    /// `prefix:*`
    NamespaceWildcard(String),
    /// `QName` (possibly namespace-prefixed local name test)
    QName(QName),
    /// `node()`
    Node,
    /// `text()`
    Text,
    /// `comment()`
    Comment,
    /// `processing-instruction()` or `processing-instruction('target')`
    ProcessingInstruction(Option<String>),
}

/// [4] `Step` — `AxisSpecifier NodeTest Predicate*`, with `AbbreviatedStep`
/// (`.` / `..`) desugared into the equivalent `self`/`parent` axis step.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub axis: Axis,
    pub node_test: NodeTest,
    pub predicates: Vec<Expr>,
}

/// [1]/[2]/[3] `LocationPath` — `//` is desugared into an explicit
/// `descendant-or-self::node()` step prepended to (or inserted into) `steps`.
#[derive(Debug, Clone, PartialEq)]
pub struct LocationPath {
    pub is_absolute: bool,
    pub steps: Vec<Step>,
}

/// [16] `FunctionCall` — name plus argument expressions. Not validated
/// against the core function library in this phase; any syntactically
/// valid `QName` is accepted.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionCall {
    pub name: QName,
    pub args: Vec<Expr>,
}

/// [15] `PrimaryExpr`.
#[derive(Debug, Clone, PartialEq)]
pub enum PrimaryExpr {
    /// [36] `VariableReference` — `$QName`.
    Variable(QName),
    /// `'(' Expr ')'`
    Parenthesized(Box<Expr>),
    /// [29] `Literal`.
    Literal(String),
    /// [30] `Number`.
    Number(f64),
    /// [16] `FunctionCall`.
    Function(FunctionCall),
}

/// [20] `FilterExpr` — a `PrimaryExpr` with zero or more predicates.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterExpr {
    pub primary: PrimaryExpr,
    pub predicates: Vec<Expr>,
}

/// [19] `PathExpr`.
#[derive(Debug, Clone, PartialEq)]
pub enum PathExpr {
    /// A bare `LocationPath`.
    Location(LocationPath),
    /// A bare `FilterExpr` (no trailing `/` or `//`).
    Filter(FilterExpr),
    /// `FilterExpr '/' RelativeLocationPath` or, with `//` desugared into a
    /// prepended `descendant-or-self::node()` step,
    /// `FilterExpr '//' RelativeLocationPath`.
    FilterLocation(FilterExpr, LocationPath),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqualityOp {
    Eq,
    Ne,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationalOp {
    Lt,
    Gt,
    Le,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdditiveOp {
    Add,
    Sub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiplicativeOp {
    Mul,
    Div,
    Mod,
}

/// [14] `Expr` and the full operator-precedence chain [18]/[21]-[27] in a
/// single flat enum: each precedence level either produces a node here or,
/// when its operator is absent, simply returns the inner expression from
/// the next-tighter level (standard precedence-climbing shortcut).
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// [21] `OrExpr`.
    Or(Box<Expr>, Box<Expr>),
    /// [22] `AndExpr`.
    And(Box<Expr>, Box<Expr>),
    /// [23] `EqualityExpr`.
    Equality(Box<Expr>, EqualityOp, Box<Expr>),
    /// [24] `RelationalExpr`.
    Relational(Box<Expr>, RelationalOp, Box<Expr>),
    /// [25] `AdditiveExpr`.
    Additive(Box<Expr>, AdditiveOp, Box<Expr>),
    /// [26] `MultiplicativeExpr`.
    Multiplicative(Box<Expr>, MultiplicativeOp, Box<Expr>),
    /// [27] `UnaryExpr ::= '-' UnaryExpr`.
    Negate(Box<Expr>),
    /// [18] `UnionExpr`.
    Union(Box<Expr>, Box<Expr>),
    /// [19] `PathExpr`.
    Path(PathExpr),
}
