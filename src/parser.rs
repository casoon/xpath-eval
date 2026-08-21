//! Recursive-descent parser over the XPath 1.0 operator-precedence chain
//! ([14],[18],[21]-[27]: Or → And → Equality → Relational → Additive →
//! Multiplicative → Unary → Union → Path → Filter → Primary), plus
//! `LocationPath`/`Step`/`Predicate` parsing ([1]-[13]).
//!
//! Function names are accepted purely syntactically (name + argument
//! list) — they are not checked against the core function library here;
//! that is Phase 04 (evaluation).

use crate::ast::{
    AdditiveOp, Axis, EqualityOp, Expr, FilterExpr, FunctionCall, LocationPath, MultiplicativeOp,
    NodeTest, PathExpr, PrimaryExpr, QName, RelationalOp, Step,
};
use crate::lexer::{Lexer, Token};

/// A structured parse error: a byte offset into the input expression plus a
/// human-readable description (not a bare string error).
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub position: usize,
    pub message: String,
}

impl ParseError {
    pub(crate) fn new(position: usize, message: impl Into<String>) -> Self {
        ParseError {
            position,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "parse error at byte {}: {}", self.position, self.message)
    }
}

impl std::error::Error for ParseError {}

const NODE_TYPES: [&str; 4] = ["comment", "text", "processing-instruction", "node"];

fn desc_or_self_step() -> Step {
    Step {
        axis: Axis::DescendantOrSelf,
        node_test: NodeTest::Node,
        predicates: Vec::new(),
    }
}

/// Parses an XPath 1.0 expression string into a structured [`Expr`] AST.
pub fn parse(expr: &str) -> Result<Expr, ParseError> {
    let tokens = Lexer::tokenize(expr)?;
    if tokens.is_empty() {
        return Err(ParseError::new(0, "empty expression"));
    }
    let end = expr.len();
    let mut parser = Parser {
        tokens,
        pos: 0,
        end,
    };
    let result = parser.parse_expr()?;
    if parser.pos != parser.tokens.len() {
        let p = parser.current_pos();
        return Err(ParseError::new(p, "unexpected trailing input"));
    }
    Ok(result)
}

struct Parser {
    tokens: Vec<(Token, usize)>,
    pos: usize,
    end: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|(t, _)| t)
    }

    fn peek_at(&self, offset: usize) -> Option<&Token> {
        self.tokens.get(self.pos + offset).map(|(t, _)| t)
    }

    fn current_pos(&self) -> usize {
        self.tokens
            .get(self.pos)
            .map(|(_, p)| *p)
            .unwrap_or(self.end)
    }

    fn advance(&mut self) -> Token {
        let (t, _) = self.tokens[self.pos].clone();
        self.pos += 1;
        t
    }

    fn expect(&mut self, tok: Token, what: &str) -> Result<(), ParseError> {
        match self.peek() {
            Some(t) if *t == tok => {
                self.pos += 1;
                Ok(())
            }
            Some(_) => Err(ParseError::new(
                self.current_pos(),
                format!("expected {what}"),
            )),
            None => Err(self.eof_error(what)),
        }
    }

    fn eof_error(&self, what: &str) -> ParseError {
        ParseError::new(
            self.end,
            format!("unexpected end of input, expected {what}"),
        )
    }

    // ---- Operator-precedence chain ---------------------------------

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_and()?;
        while matches!(self.peek(), Some(Token::Or)) {
            self.advance();
            let rhs = self.parse_and()?;
            lhs = Expr::Or(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_equality()?;
        while matches!(self.peek(), Some(Token::And)) {
            self.advance();
            let rhs = self.parse_equality()?;
            lhs = Expr::And(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_equality(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_relational()?;
        loop {
            let op = match self.peek() {
                Some(Token::Eq) => EqualityOp::Eq,
                Some(Token::Ne) => EqualityOp::Ne,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_relational()?;
            lhs = Expr::Equality(Box::new(lhs), op, Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_relational(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_additive()?;
        loop {
            let op = match self.peek() {
                Some(Token::Lt) => RelationalOp::Lt,
                Some(Token::Gt) => RelationalOp::Gt,
                Some(Token::Le) => RelationalOp::Le,
                Some(Token::Ge) => RelationalOp::Ge,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_additive()?;
            lhs = Expr::Relational(Box::new(lhs), op, Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_additive(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Some(Token::Plus) => AdditiveOp::Add,
                Some(Token::Minus) => AdditiveOp::Sub,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_multiplicative()?;
            lhs = Expr::Additive(Box::new(lhs), op, Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Some(Token::Star) => MultiplicativeOp::Mul,
                Some(Token::Div) => MultiplicativeOp::Div,
                Some(Token::Mod) => MultiplicativeOp::Mod,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_unary()?;
            lhs = Expr::Multiplicative(Box::new(lhs), op, Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        if matches!(self.peek(), Some(Token::Minus)) {
            self.advance();
            let operand = self.parse_unary()?;
            Ok(Expr::Negate(Box::new(operand)))
        } else {
            self.parse_union()
        }
    }

    fn parse_union(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_path_expr()?;
        while matches!(self.peek(), Some(Token::Pipe)) {
            self.advance();
            let rhs = self.parse_path_expr()?;
            lhs = Expr::Union(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    // ---- PathExpr / LocationPath / Step / Predicate -----------------

    fn peek_is_node_type_call(&self) -> bool {
        matches!(self.peek(), Some(Token::Name { prefix: None, local }) if NODE_TYPES.contains(&local.as_str()))
            && matches!(self.peek_at(1), Some(Token::LParen))
    }

    fn peek_axis(&self) -> Option<Axis> {
        if let Some(Token::Name {
            prefix: None,
            local,
        }) = self.peek()
            && matches!(self.peek_at(1), Some(Token::ColonColon))
        {
            return Axis::from_name(local);
        }
        None
    }

    fn starts_location_path(&self) -> bool {
        match self.peek() {
            Some(Token::Slash) | Some(Token::SlashSlash) => true,
            Some(Token::Dot)
            | Some(Token::DotDot)
            | Some(Token::At)
            | Some(Token::Wildcard)
            | Some(Token::NsWildcard(_)) => true,
            Some(Token::Name { .. }) => {
                if self.peek_is_node_type_call() {
                    true
                } else {
                    !matches!(self.peek_at(1), Some(Token::LParen))
                }
            }
            _ => false,
        }
    }

    fn can_start_step(&self) -> bool {
        matches!(
            self.peek(),
            Some(Token::Dot)
                | Some(Token::DotDot)
                | Some(Token::At)
                | Some(Token::Wildcard)
                | Some(Token::NsWildcard(_))
                | Some(Token::Name { .. })
        )
    }

    fn parse_path_expr(&mut self) -> Result<Expr, ParseError> {
        if self.starts_location_path() {
            let loc = self.parse_location_path()?;
            Ok(Expr::Path(PathExpr::Location(loc)))
        } else {
            let filter = self.parse_filter_expr()?;
            if matches!(self.peek(), Some(Token::Slash)) {
                self.advance();
                let steps = self.parse_step_sequence()?;
                Ok(Expr::Path(PathExpr::FilterLocation(
                    filter,
                    LocationPath {
                        is_absolute: false,
                        steps,
                    },
                )))
            } else if matches!(self.peek(), Some(Token::SlashSlash)) {
                self.advance();
                let mut steps = vec![desc_or_self_step()];
                steps.extend(self.parse_step_sequence()?);
                Ok(Expr::Path(PathExpr::FilterLocation(
                    filter,
                    LocationPath {
                        is_absolute: false,
                        steps,
                    },
                )))
            } else {
                Ok(Expr::Path(PathExpr::Filter(filter)))
            }
        }
    }

    fn parse_location_path(&mut self) -> Result<LocationPath, ParseError> {
        if matches!(self.peek(), Some(Token::Slash)) {
            self.advance();
            if self.can_start_step() {
                let steps = self.parse_step_sequence()?;
                Ok(LocationPath {
                    is_absolute: true,
                    steps,
                })
            } else {
                Ok(LocationPath {
                    is_absolute: true,
                    steps: Vec::new(),
                })
            }
        } else if matches!(self.peek(), Some(Token::SlashSlash)) {
            self.advance();
            let mut steps = vec![desc_or_self_step()];
            steps.extend(self.parse_step_sequence()?);
            Ok(LocationPath {
                is_absolute: true,
                steps,
            })
        } else {
            let steps = self.parse_step_sequence()?;
            Ok(LocationPath {
                is_absolute: false,
                steps,
            })
        }
    }

    fn parse_step_sequence(&mut self) -> Result<Vec<Step>, ParseError> {
        let mut steps = vec![self.parse_step()?];
        loop {
            if matches!(self.peek(), Some(Token::Slash)) {
                self.advance();
                steps.push(self.parse_step()?);
            } else if matches!(self.peek(), Some(Token::SlashSlash)) {
                self.advance();
                steps.push(desc_or_self_step());
                steps.push(self.parse_step()?);
            } else {
                break;
            }
        }
        Ok(steps)
    }

    fn parse_step(&mut self) -> Result<Step, ParseError> {
        match self.peek() {
            Some(Token::Dot) => {
                self.advance();
                return Ok(Step {
                    axis: Axis::SelfAxis,
                    node_test: NodeTest::Node,
                    predicates: Vec::new(),
                });
            }
            Some(Token::DotDot) => {
                self.advance();
                return Ok(Step {
                    axis: Axis::Parent,
                    node_test: NodeTest::Node,
                    predicates: Vec::new(),
                });
            }
            _ => {}
        }
        let axis = if matches!(self.peek(), Some(Token::At)) {
            self.advance();
            Axis::Attribute
        } else if let Some(axis) = self.peek_axis() {
            self.advance(); // AxisName
            self.advance(); // '::'
            axis
        } else {
            Axis::Child
        };
        let node_test = self.parse_node_test()?;
        let mut predicates = Vec::new();
        while matches!(self.peek(), Some(Token::LBracket)) {
            self.advance();
            let e = self.parse_expr()?;
            self.expect(Token::RBracket, "']'")?;
            predicates.push(e);
        }
        Ok(Step {
            axis,
            node_test,
            predicates,
        })
    }

    fn parse_node_test(&mut self) -> Result<NodeTest, ParseError> {
        match self.peek().cloned() {
            Some(Token::Wildcard) => {
                self.advance();
                Ok(NodeTest::AnyName)
            }
            Some(Token::NsWildcard(prefix)) => {
                self.advance();
                Ok(NodeTest::NamespaceWildcard(prefix))
            }
            Some(Token::Name { prefix, local }) => {
                if matches!(self.peek_at(1), Some(Token::LParen)) {
                    if prefix.is_none() && NODE_TYPES.contains(&local.as_str()) {
                        self.advance(); // name
                        self.advance(); // '('
                        let result = match local.as_str() {
                            "node" => {
                                self.expect(Token::RParen, "')'")?;
                                NodeTest::Node
                            }
                            "text" => {
                                self.expect(Token::RParen, "')'")?;
                                NodeTest::Text
                            }
                            "comment" => {
                                self.expect(Token::RParen, "')'")?;
                                NodeTest::Comment
                            }
                            "processing-instruction" => {
                                if let Some(Token::Literal(lit)) = self.peek().cloned() {
                                    self.advance();
                                    self.expect(Token::RParen, "')'")?;
                                    NodeTest::ProcessingInstruction(Some(lit))
                                } else {
                                    self.expect(Token::RParen, "')'")?;
                                    NodeTest::ProcessingInstruction(None)
                                }
                            }
                            _ => unreachable!("filtered by NODE_TYPES.contains above"),
                        };
                        Ok(result)
                    } else {
                        Err(ParseError::new(
                            self.current_pos(),
                            "function call is not a valid node test",
                        ))
                    }
                } else {
                    self.advance();
                    Ok(NodeTest::QName(QName { prefix, local }))
                }
            }
            Some(_) => Err(ParseError::new(self.current_pos(), "expected a node test")),
            None => Err(self.eof_error("a node test")),
        }
    }

    // ---- FilterExpr / PrimaryExpr -----------------------------------

    fn parse_filter_expr(&mut self) -> Result<FilterExpr, ParseError> {
        let primary = self.parse_primary_expr()?;
        let mut predicates = Vec::new();
        while matches!(self.peek(), Some(Token::LBracket)) {
            self.advance();
            let e = self.parse_expr()?;
            self.expect(Token::RBracket, "']'")?;
            predicates.push(e);
        }
        Ok(FilterExpr {
            primary,
            predicates,
        })
    }

    fn parse_primary_expr(&mut self) -> Result<PrimaryExpr, ParseError> {
        match self.peek().cloned() {
            Some(Token::Variable(qname)) => {
                self.advance();
                Ok(PrimaryExpr::Variable(qname))
            }
            Some(Token::LParen) => {
                self.advance();
                let e = self.parse_expr()?;
                self.expect(Token::RParen, "')'")?;
                Ok(PrimaryExpr::Parenthesized(Box::new(e)))
            }
            Some(Token::Literal(s)) => {
                self.advance();
                Ok(PrimaryExpr::Literal(s))
            }
            Some(Token::Number(n)) => {
                self.advance();
                Ok(PrimaryExpr::Number(n))
            }
            Some(Token::Name { prefix, local })
                if matches!(self.peek_at(1), Some(Token::LParen)) =>
            {
                self.advance(); // name
                self.advance(); // '('
                let args = self.parse_argument_list()?;
                self.expect(Token::RParen, "')'")?;
                Ok(PrimaryExpr::Function(FunctionCall {
                    name: QName { prefix, local },
                    args,
                }))
            }
            Some(_) => Err(ParseError::new(
                self.current_pos(),
                "expected an expression",
            )),
            None => Err(self.eof_error("an expression")),
        }
    }

    fn parse_argument_list(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        if matches!(self.peek(), Some(Token::RParen)) {
            return Ok(args);
        }
        args.push(self.parse_expr()?);
        while matches!(self.peek(), Some(Token::Comma)) {
            self.advance();
            args.push(self.parse_expr()?);
        }
        Ok(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;

    fn qn(local: &str) -> QName {
        QName {
            prefix: None,
            local: local.to_string(),
        }
    }

    fn qn_ns(prefix: &str, local: &str) -> QName {
        QName {
            prefix: Some(prefix.to_string()),
            local: local.to_string(),
        }
    }

    fn name_test(local: &str) -> NodeTest {
        NodeTest::QName(qn(local))
    }

    fn step(axis: Axis, node_test: NodeTest) -> Step {
        Step {
            axis,
            node_test,
            predicates: Vec::new(),
        }
    }

    fn rel_path(steps: Vec<Step>) -> Expr {
        Expr::Path(PathExpr::Location(LocationPath {
            is_absolute: false,
            steps,
        }))
    }

    fn abs_path(steps: Vec<Step>) -> Expr {
        Expr::Path(PathExpr::Location(LocationPath {
            is_absolute: true,
            steps,
        }))
    }

    // ---- Abbreviated vs. full axis syntax ----------------------------

    #[test]
    fn abbreviated_child_axis_matches_full_child_axis() {
        let abbrev = parse("para").unwrap();
        let full = parse("child::para").unwrap();
        let expected = rel_path(vec![step(Axis::Child, name_test("para"))]);
        assert_eq!(abbrev, expected);
        assert_eq!(full, expected);
    }

    #[test]
    fn abbreviated_attribute_axis_matches_full_attribute_axis() {
        let abbrev = parse("@name").unwrap();
        let full = parse("attribute::name").unwrap();
        let expected = rel_path(vec![step(Axis::Attribute, name_test("name"))]);
        assert_eq!(abbrev, expected);
        assert_eq!(full, expected);
    }

    #[test]
    fn dot_desugars_to_self_axis_node_test() {
        assert_eq!(
            parse(".").unwrap(),
            rel_path(vec![step(Axis::SelfAxis, NodeTest::Node)])
        );
    }

    #[test]
    fn dot_dot_desugars_to_parent_axis_node_test() {
        assert_eq!(
            parse("..").unwrap(),
            rel_path(vec![step(Axis::Parent, NodeTest::Node)])
        );
    }

    #[test]
    fn double_slash_desugars_to_descendant_or_self_node_step() {
        assert_eq!(
            parse("//para").unwrap(),
            abs_path(vec![
                step(Axis::DescendantOrSelf, NodeTest::Node),
                step(Axis::Child, name_test("para")),
            ])
        );
    }

    #[test]
    fn double_slash_mid_path_inserts_descendant_or_self_step() {
        assert_eq!(
            parse("a//b").unwrap(),
            rel_path(vec![
                step(Axis::Child, name_test("a")),
                step(Axis::DescendantOrSelf, NodeTest::Node),
                step(Axis::Child, name_test("b")),
            ])
        );
    }

    #[test]
    fn spec_example_absolute_path_with_predicates() {
        // /doc/chapter[1]/para[last()] — checks AST shape, `last()` is
        // parsed as a syntactic function call, not evaluated.
        let last_call = Expr::Path(PathExpr::Filter(FilterExpr {
            primary: PrimaryExpr::Function(FunctionCall {
                name: qn("last"),
                args: vec![],
            }),
            predicates: vec![],
        }));
        let one = Expr::Path(PathExpr::Filter(FilterExpr {
            primary: PrimaryExpr::Number(1.0),
            predicates: vec![],
        }));
        assert_eq!(
            parse("/doc/chapter[1]/para[last()]").unwrap(),
            abs_path(vec![
                step(Axis::Child, name_test("doc")),
                Step {
                    axis: Axis::Child,
                    node_test: name_test("chapter"),
                    predicates: vec![one],
                },
                Step {
                    axis: Axis::Child,
                    node_test: name_test("para"),
                    predicates: vec![last_call],
                },
            ])
        );
    }

    // ---- All 13 axes --------------------------------------------------

    #[test]
    fn all_thirteen_axes_parse_to_their_enum_variant() {
        let cases = [
            ("ancestor::a", Axis::Ancestor),
            ("ancestor-or-self::a", Axis::AncestorOrSelf),
            ("attribute::a", Axis::Attribute),
            ("child::a", Axis::Child),
            ("descendant::a", Axis::Descendant),
            ("descendant-or-self::a", Axis::DescendantOrSelf),
            ("following::a", Axis::Following),
            ("following-sibling::a", Axis::FollowingSibling),
            ("namespace::a", Axis::Namespace),
            ("parent::a", Axis::Parent),
            ("preceding::a", Axis::Preceding),
            ("preceding-sibling::a", Axis::PrecedingSibling),
            ("self::a", Axis::SelfAxis),
        ];
        for (src, axis) in cases {
            assert_eq!(
                parse(src).unwrap(),
                rel_path(vec![step(axis, name_test("a"))]),
                "axis expression {src:?}"
            );
        }
    }

    // ---- NodeType tests -------------------------------------------------

    #[test]
    fn node_type_tests() {
        assert_eq!(
            parse("text()").unwrap(),
            rel_path(vec![step(Axis::Child, NodeTest::Text)])
        );
        assert_eq!(
            parse("comment()").unwrap(),
            rel_path(vec![step(Axis::Child, NodeTest::Comment)])
        );
        assert_eq!(
            parse("node()").unwrap(),
            rel_path(vec![step(Axis::Child, NodeTest::Node)])
        );
        assert_eq!(
            parse("processing-instruction()").unwrap(),
            rel_path(vec![step(
                Axis::Child,
                NodeTest::ProcessingInstruction(None)
            )])
        );
        assert_eq!(
            parse("processing-instruction('foo')").unwrap(),
            rel_path(vec![step(
                Axis::Child,
                NodeTest::ProcessingInstruction(Some("foo".to_string()))
            )])
        );
    }

    // ---- Predicates -----------------------------------------------------

    #[test]
    fn chained_predicates() {
        let a = Step {
            axis: Axis::Child,
            node_test: name_test("a"),
            predicates: vec![
                rel_path(vec![step(Axis::Child, name_test("b"))]),
                rel_path(vec![step(Axis::Child, name_test("c"))]),
            ],
        };
        assert_eq!(parse("a[b][c]").unwrap(), rel_path(vec![a]));
    }

    #[test]
    fn nested_predicate() {
        // a[b[c]]
        let inner_b = Step {
            axis: Axis::Child,
            node_test: name_test("b"),
            predicates: vec![rel_path(vec![step(Axis::Child, name_test("c"))])],
        };
        let a = Step {
            axis: Axis::Child,
            node_test: name_test("a"),
            predicates: vec![rel_path(vec![inner_b])],
        };
        assert_eq!(parse("a[b[c]]").unwrap(), rel_path(vec![a]));
    }

    // ---- Operator precedence / associativity -----------------------------

    #[test]
    fn multiplicative_binds_tighter_than_additive() {
        // 1 + 2 * 3
        let one = Expr::Path(PathExpr::Filter(FilterExpr {
            primary: PrimaryExpr::Number(1.0),
            predicates: vec![],
        }));
        let two = Expr::Path(PathExpr::Filter(FilterExpr {
            primary: PrimaryExpr::Number(2.0),
            predicates: vec![],
        }));
        let three = Expr::Path(PathExpr::Filter(FilterExpr {
            primary: PrimaryExpr::Number(3.0),
            predicates: vec![],
        }));
        let expected = Expr::Additive(
            Box::new(one),
            AdditiveOp::Add,
            Box::new(Expr::Multiplicative(
                Box::new(two),
                MultiplicativeOp::Mul,
                Box::new(three),
            )),
        );
        assert_eq!(parse("1 + 2 * 3").unwrap(), expected);
    }

    #[test]
    fn parens_override_precedence() {
        // (1 + 2) * 3
        let one = Expr::Path(PathExpr::Filter(FilterExpr {
            primary: PrimaryExpr::Number(1.0),
            predicates: vec![],
        }));
        let two = Expr::Path(PathExpr::Filter(FilterExpr {
            primary: PrimaryExpr::Number(2.0),
            predicates: vec![],
        }));
        let three = Expr::Path(PathExpr::Filter(FilterExpr {
            primary: PrimaryExpr::Number(3.0),
            predicates: vec![],
        }));
        let sum = Expr::Additive(Box::new(one), AdditiveOp::Add, Box::new(two));
        let grouped = Expr::Path(PathExpr::Filter(FilterExpr {
            primary: PrimaryExpr::Parenthesized(Box::new(sum)),
            predicates: vec![],
        }));
        let expected =
            Expr::Multiplicative(Box::new(grouped), MultiplicativeOp::Mul, Box::new(three));
        assert_eq!(parse("(1 + 2) * 3").unwrap(), expected);
    }

    #[test]
    fn subtraction_is_left_associative() {
        // 1 - 2 - 3 == (1 - 2) - 3
        fn num(n: f64) -> Expr {
            Expr::Path(PathExpr::Filter(FilterExpr {
                primary: PrimaryExpr::Number(n),
                predicates: vec![],
            }))
        }
        let expected = Expr::Additive(
            Box::new(Expr::Additive(
                Box::new(num(1.0)),
                AdditiveOp::Sub,
                Box::new(num(2.0)),
            )),
            AdditiveOp::Sub,
            Box::new(num(3.0)),
        );
        assert_eq!(parse("1 - 2 - 3").unwrap(), expected);
    }

    #[test]
    fn unary_minus() {
        // -1 + 2
        fn num(n: f64) -> Expr {
            Expr::Path(PathExpr::Filter(FilterExpr {
                primary: PrimaryExpr::Number(n),
                predicates: vec![],
            }))
        }
        let expected = Expr::Additive(
            Box::new(Expr::Negate(Box::new(num(1.0)))),
            AdditiveOp::Add,
            Box::new(num(2.0)),
        );
        assert_eq!(parse("-1 + 2").unwrap(), expected);
    }

    // ---- div/mod/and/or as operators AND as valid NCNames -----------------

    #[test]
    fn div_mod_and_or_as_operators() {
        fn num(n: f64) -> Expr {
            Expr::Path(PathExpr::Filter(FilterExpr {
                primary: PrimaryExpr::Number(n),
                predicates: vec![],
            }))
        }
        assert_eq!(
            parse("1 div 2").unwrap(),
            Expr::Multiplicative(
                Box::new(num(1.0)),
                MultiplicativeOp::Div,
                Box::new(num(2.0))
            )
        );
        assert_eq!(
            parse("1 mod 2").unwrap(),
            Expr::Multiplicative(
                Box::new(num(1.0)),
                MultiplicativeOp::Mod,
                Box::new(num(2.0))
            )
        );
        assert_eq!(
            parse("a and b").unwrap(),
            Expr::And(
                Box::new(rel_path(vec![step(Axis::Child, name_test("a"))])),
                Box::new(rel_path(vec![step(Axis::Child, name_test("b"))])),
            )
        );
        assert_eq!(
            parse("a or b").unwrap(),
            Expr::Or(
                Box::new(rel_path(vec![step(Axis::Child, name_test("a"))])),
                Box::new(rel_path(vec![step(Axis::Child, name_test("b"))])),
            )
        );
    }

    #[test]
    fn div_as_ncname_in_non_operator_position() {
        // child::div — after '::', "div" must be a NameTest, not an operator.
        assert_eq!(
            parse("child::div").unwrap(),
            rel_path(vec![step(Axis::Child, name_test("div"))])
        );
        // //div — after '//', "div" must be a NameTest, not an operator.
        assert_eq!(
            parse("//div").unwrap(),
            abs_path(vec![
                step(Axis::DescendantOrSelf, NodeTest::Node),
                step(Axis::Child, name_test("div")),
            ])
        );
    }

    // ---- Union --------------------------------------------------------

    #[test]
    fn union_of_three_paths_is_left_associative() {
        let a = rel_path(vec![step(Axis::Child, name_test("a"))]);
        let b = rel_path(vec![step(Axis::Child, name_test("b"))]);
        let c = rel_path(vec![step(Axis::Child, name_test("c"))]);
        let expected = Expr::Union(Box::new(Expr::Union(Box::new(a), Box::new(b))), Box::new(c));
        assert_eq!(parse("a | b | c").unwrap(), expected);
    }

    // ---- Variable references -------------------------------------------

    #[test]
    fn variable_references() {
        assert_eq!(
            parse("$foo").unwrap(),
            Expr::Path(PathExpr::Filter(FilterExpr {
                primary: PrimaryExpr::Variable(qn("foo")),
                predicates: vec![],
            }))
        );
        assert_eq!(
            parse("$ns:foo").unwrap(),
            Expr::Path(PathExpr::Filter(FilterExpr {
                primary: PrimaryExpr::Variable(qn_ns("ns", "foo")),
                predicates: vec![],
            }))
        );
    }

    // ---- Literals -------------------------------------------------------

    #[test]
    fn literals_both_quote_styles() {
        assert_eq!(
            parse("'hello'").unwrap(),
            Expr::Path(PathExpr::Filter(FilterExpr {
                primary: PrimaryExpr::Literal("hello".to_string()),
                predicates: vec![],
            }))
        );
        assert_eq!(
            parse("\"hello\"").unwrap(),
            Expr::Path(PathExpr::Filter(FilterExpr {
                primary: PrimaryExpr::Literal("hello".to_string()),
                predicates: vec![],
            }))
        );
    }

    // ---- Numbers --------------------------------------------------------

    #[test]
    #[allow(clippy::approx_constant)] // 3.14 is a plain test literal, not an approximation of pi
    fn numbers() {
        fn number(n: f64) -> Expr {
            Expr::Path(PathExpr::Filter(FilterExpr {
                primary: PrimaryExpr::Number(n),
                predicates: vec![],
            }))
        }
        assert_eq!(parse("3").unwrap(), number(3.0));
        assert_eq!(parse("3.14").unwrap(), number(3.14));
        assert_eq!(parse(".5").unwrap(), number(0.5));
    }

    // ---- Namespace-prefixed name tests -----------------------------------

    #[test]
    fn namespace_prefixed_name_tests() {
        assert_eq!(
            parse("svg:rect").unwrap(),
            rel_path(vec![step(
                Axis::Child,
                NodeTest::QName(qn_ns("svg", "rect"))
            )])
        );
        assert_eq!(
            parse("*").unwrap(),
            rel_path(vec![step(Axis::Child, NodeTest::AnyName)])
        );
        assert_eq!(
            parse("svg:*").unwrap(),
            rel_path(vec![step(
                Axis::Child,
                NodeTest::NamespaceWildcard("svg".to_string())
            )])
        );
    }

    // ---- Function calls ---------------------------------------------------

    #[test]
    fn function_call_with_multiple_arguments() {
        let call = parse("concat(a, b, c)").unwrap();
        let arg = |n: &str| rel_path(vec![step(Axis::Child, name_test(n))]);
        assert_eq!(
            call,
            Expr::Path(PathExpr::Filter(FilterExpr {
                primary: PrimaryExpr::Function(FunctionCall {
                    name: qn("concat"),
                    args: vec![arg("a"), arg("b"), arg("c")],
                }),
                predicates: vec![],
            }))
        );
    }

    #[test]
    fn function_call_with_zero_arguments() {
        assert_eq!(
            parse("true()").unwrap(),
            Expr::Path(PathExpr::Filter(FilterExpr {
                primary: PrimaryExpr::Function(FunctionCall {
                    name: qn("true"),
                    args: vec![],
                }),
                predicates: vec![],
            }))
        );
    }

    // ---- Error cases ------------------------------------------------------

    #[test]
    fn error_unexpected_end_of_input() {
        let err = parse("1 +").unwrap_err();
        assert_eq!(err.position, 3);
    }

    #[test]
    fn error_unbalanced_parens() {
        let err = parse("(1 + 2").unwrap_err();
        assert_eq!(err.position, 6);

        let err2 = parse("1 + 2)").unwrap_err();
        assert_eq!(err2.position, 5);
    }

    #[test]
    fn error_empty_expression() {
        let err = parse("").unwrap_err();
        assert_eq!(err.position, 0);
        assert_eq!(err.message, "empty expression");
    }
}
