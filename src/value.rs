//! The XPath 1.0 value model (§3.1: the four object types) and the value
//! conversion functions (§4.2-§4.4), implemented against the verbatim spec
//! quotes in `plan/04-evaluation-core.md`.

use crate::document::Node;

/// An XPath 1.0 value: one of the four object types (§3.1) — "node-set
/// (an unordered collection of nodes without duplicates), boolean, number
/// ... [or] string".
///
/// Parameterized only over the node type `N`, not over a separate lifetime
/// — a concrete `N` already implements `Node<'a>` for one specific `'a`
/// (e.g. `TestNode<'a>`), so that lifetime doesn't need to be repeated here
/// (the plan's sketched `Value<'a, N: Node<'a>>` shape is adjusted to avoid
/// an otherwise-unused lifetime parameter on the enum itself).
#[derive(Clone, PartialEq)]
pub enum Value<N> {
    NodeSet(Vec<N>),
    Boolean(bool),
    /// An IEEE-754 double-precision floating point number.
    Number(f64),
    String(String),
}

// Manual `Debug` (rather than `#[derive(Debug)]`) so `Value` itself does not
// pick up an unconditional `N: Debug` bound — `Node` (`document.rs`) does not
// require `Debug`, and this type should stay usable for any conforming `N`.
impl<N: std::fmt::Debug> std::fmt::Debug for Value<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::NodeSet(nodes) => f.debug_tuple("NodeSet").field(nodes).finish(),
            Value::Boolean(b) => f.debug_tuple("Boolean").field(b).finish(),
            Value::Number(n) => f.debug_tuple("Number").field(n).finish(),
            Value::String(s) => f.debug_tuple("String").field(s).finish(),
        }
    }
}

impl<'a, N: Node<'a>> Value<N> {
    /// §4.3, verbatim: "a number is true if and only if it is neither
    /// positive or negative zero nor NaN; a node-set is true if and only if
    /// it is non-empty; a string is true if and only if its length is
    /// non-zero".
    pub fn to_boolean(&self) -> bool {
        match self {
            Value::NodeSet(nodes) => !nodes.is_empty(),
            Value::Boolean(b) => *b,
            Value::Number(n) => *n != 0.0 && !n.is_nan(),
            Value::String(s) => !s.is_empty(),
        }
    }

    /// §4.4: string→number (via the node-set→string→number chain for
    /// node-sets), boolean→number ("true is converted to 1; false to 0"),
    /// and the identity for numbers.
    pub fn to_number(&self) -> f64 {
        match self {
            Value::NodeSet(_) => string_to_number(&self.to_xpath_string()),
            Value::Boolean(b) => {
                if *b {
                    1.0
                } else {
                    0.0
                }
            }
            Value::Number(n) => *n,
            Value::String(s) => string_to_number(s),
        }
    }

    /// §4.2: node-set→string ("the string-value of the node ... that is
    /// first in document order. If the node-set is empty, an empty string
    /// is returned"), number→string (see `number_to_string`), and
    /// boolean→string ("false"/"true").
    ///
    /// Named `to_xpath_string` rather than `to_string` so this inherent
    /// method does not collide with (and get flagged by clippy as shadowing)
    /// `std::string::ToString` — `Value` deliberately does not implement
    /// `Display`/`ToString`.
    pub fn to_xpath_string(&self) -> String {
        match self {
            Value::NodeSet(nodes) => first_in_document_order(nodes)
                .map(|n| n.string_value())
                .unwrap_or_default(),
            Value::Boolean(b) => if *b { "true" } else { "false" }.to_string(),
            Value::Number(n) => number_to_string(*n),
            Value::String(s) => s.clone(),
        }
    }
}

/// The node in `nodes` that is first in document order, or `None` if empty.
/// `nodes` is a node-set (unordered per the data model), so this must not
/// assume `nodes[0]` is already the document-order-first node.
fn first_in_document_order<'a, N: Node<'a>>(nodes: &[N]) -> Option<N> {
    nodes.iter().copied().min_by(|a, b| a.document_order(*b))
}

/// §4.2, number→string, verbatim: "NaN is converted to the string NaN;
/// positive zero is converted to the string 0; negative zero is converted
/// to the string 0; positive infinity is converted to the string Infinity;
/// negative infinity is converted to the string -Infinity; if the number is
/// an integer, the number is represented in decimal form as a Number with
/// no decimal point and no leading zeros, preceded by a minus sign (-) if
/// the number is negative; otherwise, the number is represented in decimal
/// form as a Number including a decimal point with at least one digit
/// before the decimal point and at least one digit after the decimal
/// point, preceded by a minus sign (-) if the number is negative [...]".
///
/// Rust's `f64::to_string()`/`Display` already produces a correctly-rounded,
/// shortest-round-tripping decimal expansion with no exponential notation,
/// no ".0" suffix on integer-valued floats, and at least one digit on each
/// side of the point for non-integers (verified experimentally, including
/// for large/small magnitudes) — so only the special values (NaN/Infinity/
/// negative zero) need overriding here, not general-purpose formatting.
fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }
    if n == 0.0 {
        // `n == 0.0` is true for both +0.0 and -0.0 (IEEE-754), but Rust's
        // Display prints "-0" for negative zero — the spec maps both to "0".
        return "0".to_string();
    }
    n.to_string()
}

/// §4.4, string→number, verbatim: "a string that consists of optional
/// whitespace followed by an optional minus sign followed by a Number
/// followed by whitespace is converted to the IEEE 754 number that is
/// nearest ... to the mathematical value represented by the string; any
/// other string is converted to NaN".
///
/// The "Number" referred to is grammar production [30] `Number ::= Digits
/// ('.' Digits?)? | '.' Digits` — notably *not* the same as what Rust's
/// `f64::from_str` accepts (which additionally allows exponents, leading
/// `+`, `inf`/`infinity`/`nan`, etc.). This function hand-validates the
/// stricter XPath grammar first and only then hands the validated
/// digits-and-at-most-one-dot substring to `f64::from_str` for the actual
/// IEEE-754 rounding.
pub(crate) fn string_to_number(s: &str) -> f64 {
    let trimmed = s.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\r' | '\n'));
    let (negative, rest) = match trimmed.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, trimmed),
    };
    let b = rest.as_bytes();
    let mut pos = 0;
    while pos < b.len() && b[pos].is_ascii_digit() {
        pos += 1;
    }
    let has_int_digits = pos > 0;
    let mut has_frac_digits = false;
    if pos < b.len() && b[pos] == b'.' {
        pos += 1;
        let frac_start = pos;
        while pos < b.len() && b[pos].is_ascii_digit() {
            pos += 1;
        }
        has_frac_digits = pos > frac_start;
    }
    // The whole (trimmed, sign-stripped) string must be consumed, and it
    // must contain at least one digit somewhere (either side of the '.').
    if pos != b.len() || !(has_int_digits || has_frac_digits) {
        return f64::NAN;
    }
    let digits = &rest[..pos];
    let full = if negative {
        format!("-{digits}")
    } else {
        digits.to_string()
    };
    full.parse::<f64>().unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::fixture::build;

    // ---- to_boolean (§4.3) ---------------------------------------------

    #[test]
    fn boolean_number_rules() {
        assert!(!Value::<crate::document::fixture::TestNode>::Number(0.0).to_boolean());
        assert!(!Value::<crate::document::fixture::TestNode>::Number(-0.0).to_boolean());
        assert!(!Value::<crate::document::fixture::TestNode>::Number(f64::NAN).to_boolean());
        assert!(Value::<crate::document::fixture::TestNode>::Number(1.0).to_boolean());
        assert!(Value::<crate::document::fixture::TestNode>::Number(-1.0).to_boolean());
    }

    #[test]
    fn boolean_string_rules() {
        assert!(!Value::<crate::document::fixture::TestNode>::String(String::new()).to_boolean());
        assert!(Value::<crate::document::fixture::TestNode>::String("x".into()).to_boolean());
    }

    #[test]
    fn boolean_node_set_rules() {
        let f = build();
        assert!(!Value::NodeSet::<crate::document::fixture::TestNode>(Vec::new()).to_boolean());
        assert!(Value::NodeSet(vec![f.doc.node(f.span)]).to_boolean());
    }

    // ---- to_number (§4.4) -----------------------------------------------

    #[test]
    fn number_boolean_rules() {
        assert_eq!(
            Value::<crate::document::fixture::TestNode>::Boolean(true).to_number(),
            1.0
        );
        assert_eq!(
            Value::<crate::document::fixture::TestNode>::Boolean(false).to_number(),
            0.0
        );
    }

    #[test]
    fn number_string_valid_forms() {
        assert_eq!(string_to_number("42"), 42.0);
        assert_eq!(string_to_number("42.5"), 42.5);
        assert_eq!(string_to_number(".5"), 0.5);
        assert_eq!(string_to_number("5."), 5.0);
        assert_eq!(string_to_number("-42"), -42.0);
        assert_eq!(string_to_number("  42  "), 42.0);
        assert_eq!(string_to_number("\t-3.5\n"), -3.5);
    }

    #[test]
    fn number_string_malformed_is_nan() {
        assert!(string_to_number("abc").is_nan());
        assert!(string_to_number("").is_nan());
        assert!(string_to_number("--5").is_nan());
        assert!(string_to_number("1.2.3").is_nan());
        assert!(
            string_to_number("1e5").is_nan(),
            "exponent notation is not XPath Number syntax"
        );
        assert!(
            string_to_number("+5").is_nan(),
            "leading plus is not permitted"
        );
        assert!(
            string_to_number("5 6").is_nan(),
            "embedded whitespace is not permitted"
        );
        assert!(string_to_number("Infinity").is_nan());
        assert!(string_to_number("NaN").is_nan());
    }

    #[test]
    fn number_node_set_uses_first_in_document_order_string_value() {
        let f = build();
        // span_attr's string-value is "1" — a valid Number.
        let value = Value::NodeSet(vec![f.doc.node(f.span_attr)]);
        assert_eq!(value.to_number(), 1.0);
        // Order in the Vec must not matter — document order does: `comment`
        // (document-order index before `text2`) is picked even though it is
        // listed second here, and its string-value ("note") is not a Number.
        let value = Value::NodeSet(vec![f.doc.node(f.text2), f.doc.node(f.comment)]);
        assert!(
            value.to_number().is_nan(),
            "comment's string-value ('note') is not a Number"
        );
    }

    #[test]
    fn number_empty_node_set_is_nan() {
        let value = Value::NodeSet::<crate::document::fixture::TestNode>(Vec::new());
        assert!(value.to_number().is_nan());
    }

    // ---- to_xpath_string (§4.2) ------------------------------------------

    #[test]
    fn string_boolean_rules() {
        assert_eq!(
            Value::<crate::document::fixture::TestNode>::Boolean(true).to_xpath_string(),
            "true"
        );
        assert_eq!(
            Value::<crate::document::fixture::TestNode>::Boolean(false).to_xpath_string(),
            "false"
        );
    }

    #[test]
    fn string_number_special_values() {
        assert_eq!(
            Value::<crate::document::fixture::TestNode>::Number(f64::NAN).to_xpath_string(),
            "NaN"
        );
        assert_eq!(
            Value::<crate::document::fixture::TestNode>::Number(0.0).to_xpath_string(),
            "0"
        );
        assert_eq!(
            Value::<crate::document::fixture::TestNode>::Number(-0.0).to_xpath_string(),
            "0"
        );
        assert_eq!(
            Value::<crate::document::fixture::TestNode>::Number(f64::INFINITY).to_xpath_string(),
            "Infinity"
        );
        assert_eq!(
            Value::<crate::document::fixture::TestNode>::Number(f64::NEG_INFINITY)
                .to_xpath_string(),
            "-Infinity"
        );
    }

    #[test]
    #[allow(clippy::approx_constant)] // 3.14 is a plain test literal, not an approximation of pi
    fn string_number_integer_vs_decimal_formatting() {
        assert_eq!(
            Value::<crate::document::fixture::TestNode>::Number(3.0).to_xpath_string(),
            "3"
        );
        assert_eq!(
            Value::<crate::document::fixture::TestNode>::Number(-3.0).to_xpath_string(),
            "-3"
        );
        assert_eq!(
            Value::<crate::document::fixture::TestNode>::Number(100.0).to_xpath_string(),
            "100"
        );
        assert_eq!(
            Value::<crate::document::fixture::TestNode>::Number(3.14).to_xpath_string(),
            "3.14"
        );
        assert_eq!(
            Value::<crate::document::fixture::TestNode>::Number(0.5).to_xpath_string(),
            "0.5"
        );
        assert_eq!(
            Value::<crate::document::fixture::TestNode>::Number(-0.5).to_xpath_string(),
            "-0.5"
        );
    }

    #[test]
    fn string_node_set_rules() {
        let f = build();
        let value = Value::NodeSet::<crate::document::fixture::TestNode>(Vec::new());
        assert_eq!(value.to_xpath_string(), "");
        let value = Value::NodeSet(vec![f.doc.node(f.text0)]);
        assert_eq!(value.to_xpath_string(), "Hello ");
        // Vec order must not matter — document order picks the first node.
        let value = Value::NodeSet(vec![f.doc.node(f.text2), f.doc.node(f.text0)]);
        assert_eq!(value.to_xpath_string(), "Hello ");
    }
}
