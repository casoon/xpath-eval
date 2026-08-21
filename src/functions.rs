//! The remaining 24 functions of the XPath 1.0 Core Function Library (§4.1,
//! §4.2, §4.3, §4.4) — see `plan/05-function-library.md` for the verbatim
//! spec text and worked examples each implementation below is checked
//! against. `last`/`position`/`count` (§4.1) stay in `eval.rs` (Phase 04).
//!
//! `id()` relies on `Node::is_id_attribute()`, which defaults to `false`
//! everywhere — the XPath 1.0 data model derives ID-ness from a DTD/schema,
//! which most callers don't have. With no caller override, `id()` is still
//! a real, always-implemented function; it just never matches anything.

use crate::ast::{Expr, FunctionCall};
use crate::document::Node;
use crate::eval::{EvalError, EvaluationContext, evaluate};
use crate::value::Value;

fn arity_error(function: &'static str, expected: usize, got: usize) -> EvalError {
    EvalError::ArgumentCount {
        function,
        expected,
        got,
    }
}

fn check_arity(function: &'static str, args: &[Expr], expected: usize) -> Result<(), EvalError> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(arity_error(function, expected, args.len()))
    }
}

/// For functions with an optional argument (defaults to the context node) —
/// `min`/`max` is `0`/`1`.
fn check_arity_range(
    function: &'static str,
    args: &[Expr],
    min: usize,
    max: usize,
) -> Result<(), EvalError> {
    if args.len() < min {
        Err(arity_error(function, min, args.len()))
    } else if args.len() > max {
        Err(arity_error(function, max, args.len()))
    } else {
        Ok(())
    }
}

fn check_min_arity(function: &'static str, args: &[Expr], min: usize) -> Result<(), EvalError> {
    if args.len() < min {
        Err(arity_error(function, min, args.len()))
    } else {
        Ok(())
    }
}

fn as_node_set<'a, N: Node<'a>>(
    value: Value<N>,
    context: &'static str,
) -> Result<Vec<N>, EvalError> {
    match value {
        Value::NodeSet(nodes) => Ok(nodes),
        _ => Err(EvalError::ExpectedNodeSet { context }),
    }
}

fn first_in_document_order<'a, N: Node<'a>>(nodes: &[N]) -> Option<N> {
    nodes.iter().copied().min_by(|a, b| a.document_order(*b))
}

/// §4.1's shared "optional node-set argument" rule for `local-name()`/
/// `namespace-uri()`/`name()`: no argument means "a node-set containing
/// just the context node", so the first-in-document-order node is simply
/// the context node itself; with an argument, it must evaluate to a
/// node-set, and the first-in-document-order node of *that* set is used
/// (`None` if the argument's node-set is empty).
fn first_node_arg<'ctx, 'hook, N: Node<'ctx>>(
    args: &[Expr],
    ctx: &EvaluationContext<'ctx, 'hook, N>,
) -> Result<Option<N>, EvalError> {
    if args.is_empty() {
        Ok(Some(ctx.node))
    } else {
        let nodes = as_node_set(evaluate(&args[0], ctx)?, "node-set argument")?;
        Ok(first_in_document_order(&nodes))
    }
}

/// §4.2's shared "optional string argument, defaults to the context node's
/// string-value" rule for `string-length()`/`normalize-space()`.
fn string_arg_or_context<'ctx, 'hook, N: Node<'ctx>>(
    args: &[Expr],
    ctx: &EvaluationContext<'ctx, 'hook, N>,
) -> Result<String, EvalError> {
    if args.is_empty() {
        Ok(ctx.node.string_value())
    } else {
        Ok(evaluate(&args[0], ctx)?.to_xpath_string())
    }
}

/// XML's `S` production (space, tab, CR, LF) — deliberately narrower than
/// Rust's `char::is_whitespace()`, per `normalize-space()`'s spec text.
fn is_xml_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

/// §4.1, `id()`'s "whitespace-separated list of tokens" rule (referencing
/// XML's `S` production, same as `is_xml_whitespace` above).
fn whitespace_tokens(s: &str) -> Vec<String> {
    s.split(is_xml_whitespace)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// §4.1, `id()` — verbatim: "the result is a node-set containing the
/// elements in the same document as the context node that have a unique ID
/// equal to any of the tokens in the list". `root` must be that document's
/// root (see `id()`'s call site); "unique ID" is delegated entirely to
/// `Node::is_id_attribute()` — the first matching element in document order
/// is returned, since a well-formed document has at most one.
fn find_element_by_id<'a, N: Node<'a>>(root: N, id: &str) -> Option<N> {
    crate::axes::descendant_or_self(root).find(|&n| {
        n.attributes()
            .any(|a| a.is_id_attribute() && a.string_value() == id)
    })
}

/// §4.4, `round()`'s "closest to positive infinity" tie-break and negative-
/// zero special case — verbatim: "If there are two such numbers, then the
/// one that is closest to positive infinity is returned" and "If the
/// argument is less than zero, but greater than or equal to -0.5, then
/// negative zero is returned". Rust's `f64::round()` rounds ties away from
/// zero instead, so it cannot be used directly here.
fn xpath_round(n: f64) -> f64 {
    if n.is_nan() || n.is_infinite() {
        return n;
    }
    if (-0.5..0.0).contains(&n) {
        return -0.0;
    }
    (n + 0.5).floor()
}

/// §4.2, `substring(string, number, number?)` — implemented via the
/// rounded-position rule from the plan: round start/length via `round()`,
/// then a 1-based character position `p` is kept when `p >= round(start)`
/// and (no length argument, or `p < round(start) + round(length)`).
/// Comparisons/addition are plain `f64` arithmetic, so NaN/Infinity
/// propagate correctly (IEEE-754) without any special-casing here.
fn xpath_substring(s: &str, start: f64, length: Option<f64>) -> String {
    let start_r = xpath_round(start);
    let end_r = length.map(|l| start_r + xpath_round(l));
    s.chars()
        .enumerate()
        .filter_map(|(i, c)| {
            let p = (i + 1) as f64;
            let included = p >= start_r && end_r.is_none_or(|e| p < e);
            included.then_some(c)
        })
        .collect()
}

/// §4.2, `translate()` — verbatim: "If a character occurs more than once in
/// the second argument string, then the first occurrence determines the
/// replacement character. If the first argument string contains a character
/// not in the second argument string, then it is not translated." Characters
/// in the second argument with no corresponding position in the third are
/// deleted from the result.
fn xpath_translate(s: &str, from: &str, to: &str) -> String {
    let from_chars: Vec<char> = from.chars().collect();
    let to_chars: Vec<char> = to.chars().collect();
    s.chars()
        .filter_map(|c| match from_chars.iter().position(|&fc| fc == c) {
            Some(pos) => to_chars.get(pos).copied(),
            None => Some(c),
        })
        .collect()
}

/// §4.3, `lang()` — verbatim: "the language of the context node is
/// determined by the value of the xml:lang attribute on the context node,
/// or, if the context node has no xml:lang attribute, by the value of the
/// xml:lang attribute on the nearest ancestor of the context node that has
/// an xml:lang attribute." Comparison: "true if the attribute value is equal
/// to the argument ignoring case, or if there is some suffix starting with
/// '-' such that the attribute value is equal to the argument ignoring that
/// suffix […] and ignoring case."
///
/// The XML namespace URI (fixed by the XML Namespaces spec, not a
/// caller-configurable value) — `xml:lang` is only a language declaration
/// when it actually lives in this namespace; an unrelated attribute with
/// local name `"lang"` in some other namespace must not count.
const XML_NAMESPACE_URI: &str = "http://www.w3.org/XML/1998/namespace";

/// Which `xml:lang` attribute "shape" a `Node` implementer actually exposes
/// is caller-dependent (see the plan's Risiken section): a real namespace
/// split (local name `"lang"`, namespace URI either unset — a `Node` model
/// that doesn't split namespaces at all, trusted as-is — or explicitly the
/// XML namespace) or a literal, unsplit local name `"xml:lang"` (e.g.
/// `html-conform`'s `infoset.rs`). A `"lang"` local name in some other,
/// explicit namespace is rejected — that's a same-named attribute, not an
/// `xml:lang` declaration.
fn is_lang_attribute_name(en: &crate::document::ExpandedName) -> bool {
    match en.namespace_uri.as_deref() {
        Some(XML_NAMESPACE_URI) => en.local_name == "lang",
        Some(_) => false,
        None => en.local_name == "lang" || en.local_name == "xml:lang",
    }
}

fn lang_attribute_value<'a, N: Node<'a>>(n: N) -> Option<String> {
    n.attributes()
        .find(|a| {
            a.expanded_name()
                .is_some_and(|en| is_lang_attribute_name(&en))
        })
        .map(|a| a.string_value())
}

fn lang_matches(attr_value: &str, arg: &str) -> bool {
    if attr_value.eq_ignore_ascii_case(arg) {
        return true;
    }
    attr_value
        .split_once('-')
        .is_some_and(|(prefix, _suffix)| prefix.eq_ignore_ascii_case(arg))
}

fn xpath_lang<'a, N: Node<'a>>(n: N, arg: &str) -> bool {
    std::iter::once(n)
        .chain(crate::axes::ancestor(n))
        .find_map(lang_attribute_value)
        .is_some_and(|value| lang_matches(&value, arg))
}

/// §4.1's `name()`: "a string containing a QName" — the node's expanded
/// name, prefixed with its declared namespace prefix when it has a
/// namespace URI. `ExpandedName` itself carries no prefix (namespace URI +
/// local name only, §5.2 — spec-correct for an expanded name), so the
/// prefix is recovered from the `namespace` axis instead (§5.4: a namespace
/// node's `expanded_name().local_name` is the declared prefix, its
/// `string_value()` the bound URI).
///
/// An attribute node has no namespace nodes of its own (§5.3: attributes
/// inherit the owning element's in-scope bindings, they don't declare new
/// ones), so its owning element's `namespace` axis is searched instead
/// (`node.parent()`).
///
/// Falls back to the bare local name — never an error — when there is no
/// namespace URI, no matching namespace node in scope (e.g. a caller's tree
/// that doesn't populate namespace nodes), or the matching namespace node's
/// prefix is empty (the default namespace, §5.4).
fn qname_string<'a, N: Node<'a>>(node: N) -> String {
    let Some(en) = node.expanded_name() else {
        return String::new();
    };
    let Some(uri) = en.namespace_uri.as_deref() else {
        return en.local_name;
    };
    let is_attribute = node.kind() == crate::document::NodeKind::Attribute;
    let ns_owner = if is_attribute {
        node.parent()
    } else {
        Some(node)
    };
    let prefix = ns_owner.and_then(|owner| {
        // More than one namespace-axis node can share the same bound URI
        // (e.g. a default `xmlns="uri"` binding alongside an explicit
        // `xmlns:p="uri"` one) — `namespaces()`'s order is caller-defined,
        // not document order, so picking only the *first* match (as a bare
        // `.find()` would) risks landing on the empty-prefix (default)
        // binding even when a real prefix for the same URI is also in
        // scope. Default-namespace bindings never apply to attributes
        // (XML Namespaces §5.2) — an attribute can only be represented
        // with a genuine, non-empty prefix — so for attributes the empty
        // binding isn't a candidate at all.
        let candidates: Vec<String> = crate::axes::namespace(owner)
            .filter(|ns| ns.string_value() == uri)
            .filter_map(|ns| ns.expanded_name().map(|ns_name| ns_name.local_name))
            .collect();
        if is_attribute {
            candidates.into_iter().find(|p| !p.is_empty())
        } else {
            candidates
                .iter()
                .find(|p| !p.is_empty())
                .cloned()
                .or_else(|| candidates.into_iter().next())
        }
    });
    match prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}:{}", en.local_name),
        _ => en.local_name,
    }
}

/// Dispatches every core-library function *other* than `last`/`position`/
/// `count` (handled in `eval.rs`). Any name that reaches here without
/// matching one of the 24 implemented functions falls through to
/// `EvalError::UnknownFunction`, exactly like an unrecognized name would
/// before this phase.
pub(crate) fn dispatch<'ctx, 'hook, N: Node<'ctx>>(
    call: &FunctionCall,
    ctx: &EvaluationContext<'ctx, 'hook, N>,
) -> Result<Value<N>, EvalError> {
    if call.name.prefix.is_some() {
        return Err(EvalError::UnknownFunction(call.name.clone()));
    }
    let args = &call.args;
    match call.name.local.as_str() {
        // ---- §4.1 node-set functions -------------------------------------
        "id" => {
            check_arity("id", args, 1)?;
            let arg_val = evaluate(&args[0], ctx)?;
            let tokens: Vec<String> = match &arg_val {
                Value::NodeSet(nodes) => nodes
                    .iter()
                    .flat_map(|n| whitespace_tokens(&n.string_value()))
                    .collect(),
                other => whitespace_tokens(&other.to_xpath_string()),
            };
            let root = crate::eval::root_of(ctx.node);
            let mut result: Vec<N> = tokens
                .iter()
                .filter_map(|id| find_element_by_id(root, id))
                .collect();
            crate::eval::sort_dedup(&mut result);
            Ok(Value::NodeSet(result))
        }
        "local-name" => {
            check_arity_range("local-name", args, 0, 1)?;
            let node = first_node_arg(args, ctx)?;
            let name = node
                .and_then(Node::expanded_name)
                .map(|en| en.local_name)
                .unwrap_or_default();
            Ok(Value::String(name))
        }
        "namespace-uri" => {
            check_arity_range("namespace-uri", args, 0, 1)?;
            let node = first_node_arg(args, ctx)?;
            let uri = node
                .and_then(Node::expanded_name)
                .and_then(|en| en.namespace_uri)
                .unwrap_or_default();
            Ok(Value::String(uri))
        }
        "name" => {
            check_arity_range("name", args, 0, 1)?;
            let node = first_node_arg(args, ctx)?;
            let name = node.map(qname_string).unwrap_or_default();
            Ok(Value::String(name))
        }

        // ---- §4.2 string functions ---------------------------------------
        "string" => {
            check_arity_range("string", args, 0, 1)?;
            let s = if args.is_empty() {
                Value::NodeSet(vec![ctx.node]).to_xpath_string()
            } else {
                evaluate(&args[0], ctx)?.to_xpath_string()
            };
            Ok(Value::String(s))
        }
        "concat" => {
            check_min_arity("concat", args, 2)?;
            let mut out = String::new();
            for a in args {
                out.push_str(&evaluate(a, ctx)?.to_xpath_string());
            }
            Ok(Value::String(out))
        }
        "starts-with" => {
            check_arity("starts-with", args, 2)?;
            let a = evaluate(&args[0], ctx)?.to_xpath_string();
            let b = evaluate(&args[1], ctx)?.to_xpath_string();
            Ok(Value::Boolean(a.starts_with(&b)))
        }
        "contains" => {
            check_arity("contains", args, 2)?;
            let a = evaluate(&args[0], ctx)?.to_xpath_string();
            let b = evaluate(&args[1], ctx)?.to_xpath_string();
            Ok(Value::Boolean(a.contains(&b)))
        }
        "substring-before" => {
            check_arity("substring-before", args, 2)?;
            let a = evaluate(&args[0], ctx)?.to_xpath_string();
            let b = evaluate(&args[1], ctx)?.to_xpath_string();
            let result = a.find(&b).map(|i| a[..i].to_string()).unwrap_or_default();
            Ok(Value::String(result))
        }
        "substring-after" => {
            check_arity("substring-after", args, 2)?;
            let a = evaluate(&args[0], ctx)?.to_xpath_string();
            let b = evaluate(&args[1], ctx)?.to_xpath_string();
            let result = a
                .find(&b)
                .map(|i| a[i + b.len()..].to_string())
                .unwrap_or_default();
            Ok(Value::String(result))
        }
        "substring" => {
            check_arity_range("substring", args, 2, 3)?;
            let s = evaluate(&args[0], ctx)?.to_xpath_string();
            let start = evaluate(&args[1], ctx)?.to_number();
            let length = if args.len() == 3 {
                Some(evaluate(&args[2], ctx)?.to_number())
            } else {
                None
            };
            Ok(Value::String(xpath_substring(&s, start, length)))
        }
        "string-length" => {
            check_arity_range("string-length", args, 0, 1)?;
            let s = string_arg_or_context(args, ctx)?;
            Ok(Value::Number(s.chars().count() as f64))
        }
        "normalize-space" => {
            check_arity_range("normalize-space", args, 0, 1)?;
            let s = string_arg_or_context(args, ctx)?;
            let trimmed = s.trim_matches(is_xml_whitespace);
            let mut out = String::with_capacity(trimmed.len());
            let mut in_ws = false;
            for c in trimmed.chars() {
                if is_xml_whitespace(c) {
                    if !in_ws {
                        out.push(' ');
                        in_ws = true;
                    }
                } else {
                    out.push(c);
                    in_ws = false;
                }
            }
            Ok(Value::String(out))
        }
        "translate" => {
            check_arity("translate", args, 3)?;
            let s = evaluate(&args[0], ctx)?.to_xpath_string();
            let from = evaluate(&args[1], ctx)?.to_xpath_string();
            let to = evaluate(&args[2], ctx)?.to_xpath_string();
            Ok(Value::String(xpath_translate(&s, &from, &to)))
        }

        // ---- §4.3 boolean functions ---------------------------------------
        "boolean" => {
            check_arity("boolean", args, 1)?;
            Ok(Value::Boolean(evaluate(&args[0], ctx)?.to_boolean()))
        }
        "not" => {
            check_arity("not", args, 1)?;
            Ok(Value::Boolean(!evaluate(&args[0], ctx)?.to_boolean()))
        }
        "true" => {
            check_arity("true", args, 0)?;
            Ok(Value::Boolean(true))
        }
        "false" => {
            check_arity("false", args, 0)?;
            Ok(Value::Boolean(false))
        }
        "lang" => {
            check_arity("lang", args, 1)?;
            let arg = evaluate(&args[0], ctx)?.to_xpath_string();
            Ok(Value::Boolean(xpath_lang(ctx.node, &arg)))
        }

        // ---- §4.4 number functions ------------------------------------------
        "number" => {
            check_arity_range("number", args, 0, 1)?;
            let n = if args.is_empty() {
                Value::NodeSet(vec![ctx.node]).to_number()
            } else {
                evaluate(&args[0], ctx)?.to_number()
            };
            Ok(Value::Number(n))
        }
        "sum" => {
            check_arity("sum", args, 1)?;
            let nodes = as_node_set(evaluate(&args[0], ctx)?, "sum() argument")?;
            let total: f64 = nodes
                .iter()
                .map(|n| Value::<N>::String(n.string_value()).to_number())
                .sum();
            Ok(Value::Number(total))
        }
        "floor" => {
            check_arity("floor", args, 1)?;
            Ok(Value::Number(evaluate(&args[0], ctx)?.to_number().floor()))
        }
        "ceiling" => {
            check_arity("ceiling", args, 1)?;
            Ok(Value::Number(evaluate(&args[0], ctx)?.to_number().ceil()))
        }
        "round" => {
            check_arity("round", args, 1)?;
            Ok(Value::Number(xpath_round(
                evaluate(&args[0], ctx)?.to_number(),
            )))
        }

        _ => Err(EvalError::UnknownFunction(call.name.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::fixture::{Fixture, TestNode, build};
    use crate::document::{ExpandedName, NodeKind};

    fn eval_str<'a>(src: &str, node: TestNode<'a>) -> Result<Value<TestNode<'a>>, EvalError> {
        let expr = crate::parse(src).expect("test expression must parse");
        let ctx = EvaluationContext::new(node);
        crate::evaluate(&expr, &ctx)
    }

    fn root(f: &Fixture) -> TestNode<'_> {
        f.doc.node(f.html)
    }

    fn string_of(v: Result<Value<TestNode<'_>>, EvalError>) -> String {
        match v.expect("expected Ok") {
            Value::String(s) => s,
            other => panic!("expected a string, got {other:?}"),
        }
    }

    fn number_of(v: Result<Value<TestNode<'_>>, EvalError>) -> f64 {
        match v.expect("expected Ok") {
            Value::Number(n) => n,
            other => panic!("expected a number, got {other:?}"),
        }
    }

    fn boolean_of(v: Result<Value<TestNode<'_>>, EvalError>) -> bool {
        match v.expect("expected Ok") {
            Value::Boolean(b) => b,
            other => panic!("expected a boolean, got {other:?}"),
        }
    }

    // ---- §4.1 node-set functions ------------------------------------------

    #[test]
    fn local_name_defaults_to_context_node() {
        let f = build();
        assert_eq!(
            string_of(eval_str("local-name()", f.doc.node(f.span))),
            "span"
        );
    }

    #[test]
    fn local_name_with_explicit_node_set_argument() {
        let f = build();
        assert_eq!(string_of(eval_str("local-name(/html/@id)", root(&f))), "id");
    }

    #[test]
    fn local_name_is_empty_for_node_with_no_expanded_name_or_empty_node_set() {
        let f = build();
        // text0 (a text node) has no expanded name.
        assert_eq!(string_of(eval_str("local-name()", f.doc.node(f.text0))), "");
        // An argument node-set that evaluates to empty.
        assert_eq!(
            string_of(eval_str("local-name(/html/nonexistent)", root(&f))),
            ""
        );
    }

    #[test]
    fn namespace_uri_defaults_to_context_node_and_argument_form() {
        let f = build();
        // The shared fixture never populates a non-`None` namespace URI, so
        // both forms are empty here — `namespace_uri_of_a_real_namespace`
        // below covers the non-empty branch with a dedicated node.
        assert_eq!(string_of(eval_str("namespace-uri()", root(&f))), "");
        assert_eq!(
            string_of(eval_str("namespace-uri(/html/@id)", root(&f))),
            ""
        );
    }

    /// A single node carrying a real, non-`None` namespace URI — the shared
    /// `document::fixture` tree never populates one (see its own tests'
    /// precedent, `eval.rs`'s `NsTestNode`), so a minimal standalone `Node`
    /// impl is used instead of extending that fixture (out of scope here,
    /// touching `document.rs`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct NsNode;

    impl<'a> Node<'a> for NsNode {
        fn kind(self) -> NodeKind {
            NodeKind::Element
        }
        fn parent(self) -> Option<Self> {
            None
        }
        fn children(self) -> impl Iterator<Item = Self> + 'a {
            std::iter::empty()
        }
        fn attributes(self) -> impl Iterator<Item = Self> + 'a {
            std::iter::empty()
        }
        fn namespaces(self) -> impl Iterator<Item = Self> + 'a {
            std::iter::empty()
        }
        fn expanded_name(self) -> Option<ExpandedName> {
            Some(ExpandedName {
                namespace_uri: Some("http://example/ns".to_string()),
                local_name: "rect".to_string(),
            })
        }
        fn string_value(self) -> String {
            String::new()
        }
        fn document_order(self, _other: Self) -> std::cmp::Ordering {
            std::cmp::Ordering::Equal
        }
    }

    #[test]
    fn namespace_uri_of_a_real_namespace() {
        let ctx = EvaluationContext::new(NsNode);
        let expr = crate::parse("namespace-uri()").unwrap();
        assert_eq!(
            crate::evaluate(&expr, &ctx),
            Ok(Value::String("http://example/ns".to_string()))
        );
    }

    #[test]
    fn name_defaults_to_context_node_and_argument_form() {
        let f = build();
        assert_eq!(string_of(eval_str("name()", f.doc.node(f.span))), "span");
        assert_eq!(string_of(eval_str("name(/html/@id)", root(&f))), "id");
    }

    /// A minimal parent-linked tree, purpose-built to test `qname_string`'s
    /// prefix reconstruction via the `namespace` axis — the shared
    /// `document::fixture` tree never sets a non-`None` namespace URI on an
    /// element (see `NsNode`'s own doc comment above), so it can't exercise
    /// any of these cases without touching `document.rs` (out of scope
    /// here). A standalone `Node` impl is used instead, following the same
    /// precedent as `NsNode`/`LangNode`.
    #[derive(Debug)]
    enum QNameEntry {
        Element {
            namespace_uri: Option<&'static str>,
            local_name: &'static str,
            namespaces: Vec<usize>,
            attributes: Vec<usize>,
        },
        Namespace {
            prefix: &'static str,
            uri: &'static str,
        },
        Attribute {
            owner: usize,
            namespace_uri: Option<&'static str>,
            local_name: &'static str,
        },
    }

    #[derive(Debug)]
    struct QNameArena(Vec<QNameEntry>);

    #[derive(Clone, Copy, Debug)]
    struct QNameNode<'a> {
        arena: &'a QNameArena,
        idx: usize,
    }

    impl<'a> PartialEq for QNameNode<'a> {
        fn eq(&self, other: &Self) -> bool {
            std::ptr::eq(self.arena, other.arena) && self.idx == other.idx
        }
    }
    impl<'a> Eq for QNameNode<'a> {}

    impl<'a> Node<'a> for QNameNode<'a> {
        fn kind(self) -> NodeKind {
            match &self.arena.0[self.idx] {
                QNameEntry::Element { .. } => NodeKind::Element,
                QNameEntry::Namespace { .. } => NodeKind::Namespace,
                QNameEntry::Attribute { .. } => NodeKind::Attribute,
            }
        }
        fn parent(self) -> Option<Self> {
            match &self.arena.0[self.idx] {
                QNameEntry::Element { .. } => None,
                QNameEntry::Namespace { .. } => None,
                QNameEntry::Attribute { owner, .. } => Some(QNameNode {
                    arena: self.arena,
                    idx: *owner,
                }),
            }
        }
        fn children(self) -> impl Iterator<Item = Self> + 'a {
            std::iter::empty()
        }
        fn attributes(self) -> impl Iterator<Item = Self> + 'a {
            let arena = self.arena;
            let indices = match &arena.0[self.idx] {
                QNameEntry::Element { attributes, .. } => attributes.clone(),
                _ => Vec::new(),
            };
            indices
                .into_iter()
                .map(move |i| QNameNode { arena, idx: i })
        }
        fn namespaces(self) -> impl Iterator<Item = Self> + 'a {
            let arena = self.arena;
            let indices = match &arena.0[self.idx] {
                QNameEntry::Element { namespaces, .. } => namespaces.clone(),
                _ => Vec::new(),
            };
            indices
                .into_iter()
                .map(move |i| QNameNode { arena, idx: i })
        }
        fn expanded_name(self) -> Option<ExpandedName> {
            match &self.arena.0[self.idx] {
                QNameEntry::Element {
                    namespace_uri,
                    local_name,
                    ..
                } => Some(ExpandedName {
                    namespace_uri: namespace_uri.map(str::to_string),
                    local_name: local_name.to_string(),
                }),
                QNameEntry::Namespace { prefix, .. } => Some(ExpandedName {
                    namespace_uri: None,
                    local_name: prefix.to_string(),
                }),
                QNameEntry::Attribute {
                    namespace_uri,
                    local_name,
                    ..
                } => Some(ExpandedName {
                    namespace_uri: namespace_uri.map(str::to_string),
                    local_name: local_name.to_string(),
                }),
            }
        }
        fn string_value(self) -> String {
            match &self.arena.0[self.idx] {
                QNameEntry::Namespace { uri, .. } => uri.to_string(),
                _ => String::new(),
            }
        }
        fn document_order(self, other: Self) -> std::cmp::Ordering {
            self.idx.cmp(&other.idx)
        }
    }

    #[test]
    fn name_prefixes_with_the_matching_namespace_node_from_the_namespace_axis() {
        let arena = QNameArena(vec![
            QNameEntry::Element {
                namespace_uri: Some("http://example/ns1"),
                local_name: "rect",
                namespaces: vec![1],
                attributes: vec![],
            },
            QNameEntry::Namespace {
                prefix: "ns1",
                uri: "http://example/ns1",
            },
        ]);
        let node = QNameNode {
            arena: &arena,
            idx: 0,
        };
        let ctx = EvaluationContext::new(node);
        let expr = crate::parse("name()").unwrap();
        assert_eq!(
            crate::evaluate(&expr, &ctx),
            Ok(Value::String("ns1:rect".to_string()))
        );
    }

    #[test]
    fn name_has_no_leading_colon_when_the_matching_namespace_node_is_the_default_namespace() {
        let arena = QNameArena(vec![
            QNameEntry::Element {
                namespace_uri: Some("http://example/default"),
                local_name: "rect",
                namespaces: vec![1],
                attributes: vec![],
            },
            QNameEntry::Namespace {
                prefix: "",
                uri: "http://example/default",
            },
        ]);
        let node = QNameNode {
            arena: &arena,
            idx: 0,
        };
        let ctx = EvaluationContext::new(node);
        let expr = crate::parse("name()").unwrap();
        assert_eq!(
            crate::evaluate(&expr, &ctx),
            Ok(Value::String("rect".to_string()))
        );
    }

    #[test]
    fn name_of_an_attribute_uses_its_parent_elements_namespace_axis() {
        let arena = QNameArena(vec![
            QNameEntry::Element {
                namespace_uri: None,
                local_name: "rect",
                namespaces: vec![1],
                attributes: vec![2],
            },
            QNameEntry::Namespace {
                prefix: "ns2",
                uri: "http://example/ns2",
            },
            QNameEntry::Attribute {
                owner: 0,
                namespace_uri: Some("http://example/ns2"),
                local_name: "attr",
            },
        ]);
        let attribute = QNameNode {
            arena: &arena,
            idx: 2,
        };
        let ctx = EvaluationContext::new(attribute);
        let expr = crate::parse("name()").unwrap();
        assert_eq!(
            crate::evaluate(&expr, &ctx),
            Ok(Value::String("ns2:attr".to_string()))
        );
    }

    #[test]
    fn name_falls_back_to_local_name_when_no_namespace_node_matches() {
        let arena = QNameArena(vec![QNameEntry::Element {
            namespace_uri: Some("http://example/unmatched"),
            local_name: "rect",
            namespaces: vec![],
            attributes: vec![],
        }]);
        let node = QNameNode {
            arena: &arena,
            idx: 0,
        };
        let ctx = EvaluationContext::new(node);
        let expr = crate::parse("name()").unwrap();
        assert_eq!(
            crate::evaluate(&expr, &ctx),
            Ok(Value::String("rect".to_string()))
        );
    }

    #[test]
    fn name_of_an_attribute_prefers_a_real_prefix_over_a_same_uri_default_namespace_binding() {
        // Regression for the todo.md item: the default (empty-prefix)
        // binding for the attribute's URI is listed *first* on the
        // namespace axis, with the real prefix binding second — a naive
        // `.find()` of the first match would wrongly pick the empty
        // prefix and fall back to an unprefixed name. Default-namespace
        // bindings never apply to attributes (XML Namespaces §5.2), so
        // this must resolve to the real prefix instead.
        let arena = QNameArena(vec![
            QNameEntry::Element {
                namespace_uri: None,
                local_name: "rect",
                namespaces: vec![1, 2],
                attributes: vec![3],
            },
            QNameEntry::Namespace {
                prefix: "",
                uri: "http://example/ns2",
            },
            QNameEntry::Namespace {
                prefix: "ns2",
                uri: "http://example/ns2",
            },
            QNameEntry::Attribute {
                owner: 0,
                namespace_uri: Some("http://example/ns2"),
                local_name: "attr",
            },
        ]);
        let attribute = QNameNode {
            arena: &arena,
            idx: 3,
        };
        let ctx = EvaluationContext::new(attribute);
        let expr = crate::parse("name()").unwrap();
        assert_eq!(
            crate::evaluate(&expr, &ctx),
            Ok(Value::String("ns2:attr".to_string()))
        );
    }

    #[test]
    fn name_of_an_attribute_falls_back_to_local_name_when_only_a_default_namespace_binding_matches()
    {
        // Unlike an element, an attribute has no legitimate unprefixed
        // representation of a namespaced name — but if that's all the
        // namespace axis offers, falling back to the bare local name is
        // still the best available answer (not an error case this crate
        // invents a prefix for).
        let arena = QNameArena(vec![
            QNameEntry::Element {
                namespace_uri: None,
                local_name: "rect",
                namespaces: vec![1],
                attributes: vec![2],
            },
            QNameEntry::Namespace {
                prefix: "",
                uri: "http://example/ns3",
            },
            QNameEntry::Attribute {
                owner: 0,
                namespace_uri: Some("http://example/ns3"),
                local_name: "attr",
            },
        ]);
        let attribute = QNameNode {
            arena: &arena,
            idx: 2,
        };
        let ctx = EvaluationContext::new(attribute);
        let expr = crate::parse("name()").unwrap();
        assert_eq!(
            crate::evaluate(&expr, &ctx),
            Ok(Value::String("attr".to_string()))
        );
    }

    // ---- §4.2 string functions --------------------------------------------

    #[test]
    fn string_defaults_to_context_node_string_value() {
        let f = build();
        assert_eq!(
            string_of(eval_str("string()", f.doc.node(f.body))),
            "Hello World!"
        );
    }

    #[test]
    fn string_converts_explicit_argument() {
        let f = build();
        assert_eq!(string_of(eval_str("string(1 = 1)", root(&f))), "true");
        assert_eq!(string_of(eval_str("string(3)", root(&f))), "3");
    }

    #[test]
    fn concat_joins_two_and_more_arguments() {
        let f = build();
        assert_eq!(string_of(eval_str("concat('a', 'b')", root(&f))), "ab");
        assert_eq!(
            string_of(eval_str("concat('a', 'b', 'c', 'd')", root(&f))),
            "abcd"
        );
    }

    #[test]
    fn concat_with_fewer_than_two_arguments_is_an_argument_count_error() {
        let f = build();
        assert!(matches!(
            eval_str("concat('a')", root(&f)),
            Err(EvalError::ArgumentCount {
                function: "concat",
                expected: 2,
                got: 1
            })
        ));
    }

    #[test]
    fn starts_with_and_contains() {
        let f = build();
        assert!(boolean_of(eval_str(
            "starts-with('abcdef', 'abc')",
            root(&f)
        )));
        assert!(!boolean_of(eval_str(
            "starts-with('abcdef', 'xyz')",
            root(&f)
        )));
        assert!(boolean_of(eval_str("contains('abcdef', 'cd')", root(&f))));
        assert!(!boolean_of(eval_str("contains('abcdef', 'xyz')", root(&f))));
    }

    #[test]
    fn substring_before_and_after() {
        let f = build();
        assert_eq!(
            string_of(eval_str("substring-before('1999/04/01', '/')", root(&f))),
            "1999"
        );
        assert_eq!(
            string_of(eval_str("substring-after('1999/04/01', '/')", root(&f))),
            "04/01"
        );
        // Not found: empty string.
        assert_eq!(
            string_of(eval_str("substring-before('abc', 'x')", root(&f))),
            ""
        );
        assert_eq!(
            string_of(eval_str("substring-after('abc', 'x')", root(&f))),
            ""
        );
    }

    // ---- substring(): the plan's 8 literal spec examples -------------------

    #[test]
    fn substring_spec_examples() {
        let f = build();
        assert_eq!(
            string_of(eval_str("substring('12345', 2, 3)", root(&f))),
            "234"
        );
        assert_eq!(
            string_of(eval_str("substring('12345', 2)", root(&f))),
            "2345"
        );
        assert_eq!(
            string_of(eval_str("substring('12345', 1.5, 2.6)", root(&f))),
            "234"
        );
        assert_eq!(
            string_of(eval_str("substring('12345', 0, 3)", root(&f))),
            "12"
        );
        assert_eq!(
            string_of(eval_str("substring('12345', 0 div 0, 3)", root(&f))),
            ""
        );
        assert_eq!(
            string_of(eval_str("substring('12345', 1, 0 div 0)", root(&f))),
            ""
        );
        assert_eq!(
            string_of(eval_str("substring('12345', -42, 1 div 0)", root(&f))),
            "12345"
        );
        assert_eq!(
            string_of(eval_str("substring('12345', -1 div 0, 1 div 0)", root(&f))),
            ""
        );
    }

    #[test]
    fn substring_with_wrong_arity_is_an_error() {
        let f = build();
        assert!(matches!(
            eval_str("substring('12345')", root(&f)),
            Err(EvalError::ArgumentCount {
                function: "substring",
                expected: 2,
                got: 1
            })
        ));
    }

    #[test]
    fn string_length_counts_unicode_scalar_values_not_bytes() {
        let f = build();
        // "héllo" is 5 chars but 6 bytes (é is 2 bytes in UTF-8) — a
        // `.len()`-based implementation would wrongly report 6.
        assert_eq!(number_of(eval_str("string-length('héllo')", root(&f))), 5.0);
    }

    #[test]
    fn string_length_defaults_to_context_node_string_value() {
        let f = build();
        // body's string-value is "Hello World!" (12 chars).
        assert_eq!(
            number_of(eval_str("string-length()", f.doc.node(f.body))),
            12.0
        );
    }

    #[test]
    fn normalize_space_collapses_and_trims_xml_whitespace() {
        let f = build();
        assert_eq!(
            string_of(eval_str(
                "normalize-space('  \t a \r\n\r\n b  \t')",
                root(&f)
            )),
            "a b"
        );
    }

    #[test]
    fn normalize_space_defaults_to_context_node_string_value() {
        let f = build();
        assert_eq!(
            string_of(eval_str("normalize-space()", f.doc.node(f.body))),
            "Hello World!"
        );
    }

    // ---- translate(): the plan's 2 literal spec examples --------------------

    #[test]
    fn translate_spec_examples() {
        let f = build();
        assert_eq!(
            string_of(eval_str("translate('bar', 'abc', 'ABC')", root(&f))),
            "BAr"
        );
        assert_eq!(
            string_of(eval_str("translate('--aaa--', 'abc-', 'ABC')", root(&f))),
            "AAA"
        );
    }

    // ---- §4.3 boolean functions --------------------------------------------

    #[test]
    fn boolean_converts_via_to_boolean() {
        let f = build();
        assert!(boolean_of(eval_str("boolean(1)", root(&f))));
        assert!(!boolean_of(eval_str("boolean(0)", root(&f))));
        assert!(boolean_of(eval_str("boolean('x')", root(&f))));
        assert!(!boolean_of(eval_str("boolean('')", root(&f))));
    }

    #[test]
    fn not_negates() {
        let f = build();
        assert!(boolean_of(eval_str("not(false())", root(&f))));
        assert!(!boolean_of(eval_str("not(true())", root(&f))));
    }

    #[test]
    fn true_and_false_constants() {
        let f = build();
        assert!(boolean_of(eval_str("true()", root(&f))));
        assert!(!boolean_of(eval_str("false()", root(&f))));
    }

    #[test]
    fn true_with_arguments_is_an_argument_count_error() {
        let f = build();
        assert!(matches!(
            eval_str("true(1)", root(&f)),
            Err(EvalError::ArgumentCount {
                function: "true",
                expected: 0,
                got: 1
            })
        ));
    }

    // ---- lang() -------------------------------------------------------------

    #[test]
    fn lang_matches_the_shared_fixtures_html_lang_attribute_by_inheritance() {
        let f = build();
        // html has lang="en"; body/span/text nodes have no lang attribute
        // of their own, so they must inherit it from their nearest
        // ancestor that has one.
        assert!(boolean_of(eval_str("lang('en')", f.doc.node(f.body))));
        assert!(boolean_of(eval_str("lang('EN')", f.doc.node(f.span))));
        assert!(!boolean_of(eval_str("lang('de')", f.doc.node(f.body))));
    }

    #[test]
    fn lang_matches_on_the_context_node_itself() {
        let f = build();
        assert!(boolean_of(eval_str("lang('en')", f.doc.node(f.html))));
    }

    /// A minimal parent-linked tree, purpose-built to test `lang()`'s
    /// ancestor-search, suffix rule, no-match case, and the literal
    /// `"xml:lang"` local-name form — none of which the shared
    /// `document::fixture` tree can exercise (it only ever has a plain
    /// `lang="en"` attribute on `html`, with no suffix and no "no match
    /// anywhere" case). A standalone `Node` impl is used instead of
    /// extending that fixture (out of scope here, touching `document.rs`).
    enum LangEntry {
        Element {
            parent: Option<usize>,
            attribute: Option<usize>,
        },
        Attribute {
            owner: usize,
            local_name: &'static str,
            value: &'static str,
        },
    }

    struct LangArena(Vec<LangEntry>);

    #[derive(Clone, Copy)]
    struct LangNode<'a> {
        arena: &'a LangArena,
        idx: usize,
    }

    impl<'a> PartialEq for LangNode<'a> {
        fn eq(&self, other: &Self) -> bool {
            std::ptr::eq(self.arena, other.arena) && self.idx == other.idx
        }
    }
    impl<'a> Eq for LangNode<'a> {}

    impl<'a> Node<'a> for LangNode<'a> {
        fn kind(self) -> NodeKind {
            match &self.arena.0[self.idx] {
                LangEntry::Element { .. } => NodeKind::Element,
                LangEntry::Attribute { .. } => NodeKind::Attribute,
            }
        }
        fn parent(self) -> Option<Self> {
            match &self.arena.0[self.idx] {
                LangEntry::Element { parent, .. } => parent.map(|p| LangNode {
                    arena: self.arena,
                    idx: p,
                }),
                LangEntry::Attribute { owner, .. } => Some(LangNode {
                    arena: self.arena,
                    idx: *owner,
                }),
            }
        }
        fn children(self) -> impl Iterator<Item = Self> + 'a {
            std::iter::empty()
        }
        fn attributes(self) -> impl Iterator<Item = Self> + 'a {
            let arena = self.arena;
            match &arena.0[self.idx] {
                LangEntry::Element { attribute, .. } => {
                    (*attribute).map(|a| LangNode { arena, idx: a }).into_iter()
                }
                LangEntry::Attribute { .. } => None.into_iter(),
            }
        }
        fn namespaces(self) -> impl Iterator<Item = Self> + 'a {
            std::iter::empty()
        }
        fn expanded_name(self) -> Option<ExpandedName> {
            match &self.arena.0[self.idx] {
                LangEntry::Element { .. } => None,
                LangEntry::Attribute { local_name, .. } => Some(ExpandedName {
                    namespace_uri: None,
                    local_name: local_name.to_string(),
                }),
            }
        }
        fn string_value(self) -> String {
            match &self.arena.0[self.idx] {
                LangEntry::Element { .. } => String::new(),
                LangEntry::Attribute { value, .. } => value.to_string(),
            }
        }
        fn document_order(self, other: Self) -> std::cmp::Ordering {
            self.idx.cmp(&other.idx)
        }
    }

    /// Builds a root-to-leaf chain of elements, each optionally carrying one
    /// lang-shaped attribute, and returns the leaf (deepest) node.
    fn lang_chain(chain: &[(Option<&'static str>, Option<&'static str>)]) -> (LangArena, usize) {
        let mut nodes = Vec::new();
        let mut parent = None;
        for &(attr_local_name, attr_value) in chain {
            let el_idx = nodes.len();
            nodes.push(LangEntry::Element {
                parent,
                attribute: None,
            });
            if let (Some(local_name), Some(value)) = (attr_local_name, attr_value) {
                let attr_idx = nodes.len();
                nodes.push(LangEntry::Attribute {
                    owner: el_idx,
                    local_name,
                    value,
                });
                if let LangEntry::Element { attribute, .. } = &mut nodes[el_idx] {
                    *attribute = Some(attr_idx);
                }
            }
            parent = Some(el_idx);
        }
        let leaf = parent.expect("chain must be non-empty");
        (LangArena(nodes), leaf)
    }

    #[test]
    fn lang_own_attribute_exact_match_case_insensitive() {
        let (arena, leaf) = lang_chain(&[(Some("lang"), Some("en-US"))]);
        let node = LangNode {
            arena: &arena,
            idx: leaf,
        };
        assert!(super::xpath_lang(node, "EN-us"));
    }

    #[test]
    fn lang_suffix_rule_matches_language_subtag_ignoring_region() {
        let (arena, leaf) = lang_chain(&[(Some("lang"), Some("en-US"))]);
        let node = LangNode {
            arena: &arena,
            idx: leaf,
        };
        assert!(super::xpath_lang(node, "en"));
        assert!(!super::xpath_lang(node, "en-US-extra"));
    }

    #[test]
    fn lang_inherits_from_nearest_ancestor_with_the_attribute() {
        let (arena, leaf) = lang_chain(&[(Some("lang"), Some("de")), (None, None)]);
        let node = LangNode {
            arena: &arena,
            idx: leaf,
        };
        assert!(super::xpath_lang(node, "de"));
    }

    #[test]
    fn lang_no_match_anywhere_in_the_ancestor_chain() {
        let (arena, leaf) = lang_chain(&[(None, None), (None, None)]);
        let node = LangNode {
            arena: &arena,
            idx: leaf,
        };
        assert!(!super::xpath_lang(node, "en"));
    }

    #[test]
    fn lang_matches_the_literal_xml_lang_local_name_form() {
        let (arena, leaf) = lang_chain(&[(Some("xml:lang"), Some("fr"))]);
        let node = LangNode {
            arena: &arena,
            idx: leaf,
        };
        assert!(super::xpath_lang(node, "fr"));
    }

    #[test]
    fn lang_matches_when_the_attribute_is_explicitly_in_the_xml_namespace() {
        let en = ExpandedName {
            namespace_uri: Some(super::XML_NAMESPACE_URI.to_string()),
            local_name: "lang".to_string(),
        };
        assert!(super::is_lang_attribute_name(&en));
    }

    #[test]
    fn lang_rejects_a_lang_local_name_in_an_unrelated_explicit_namespace() {
        // Regression for the todo.md item: a `local-name() = "lang"`
        // attribute in some other, explicit namespace is a same-named
        // attribute, not an `xml:lang` declaration — it must not count.
        let en = ExpandedName {
            namespace_uri: Some("http://example/other".to_string()),
            local_name: "lang".to_string(),
        };
        assert!(!super::is_lang_attribute_name(&en));
    }

    // ---- §4.4 number functions ----------------------------------------------

    #[test]
    fn number_defaults_to_context_node_and_argument_form() {
        let f = build();
        assert_eq!(
            number_of(eval_str("number()", f.doc.node(f.span_attr))),
            1.0
        );
        assert_eq!(number_of(eval_str("number('42')", root(&f))), 42.0);
        assert_eq!(number_of(eval_str("number(true())", root(&f))), 1.0);
    }

    #[test]
    fn sum_adds_string_values_converted_to_number() {
        let f = build();
        // html/@id has string-value "root-el" (NaN), so sum over just
        // span's data-x ("1") is used instead to keep this a clean numeric
        // sum; body/@class ("main") would also be NaN.
        assert_eq!(
            number_of(eval_str("sum(/html/body/span/@data-x)", root(&f))),
            1.0
        );
    }

    #[test]
    fn sum_of_non_node_set_is_an_error() {
        let f = build();
        assert!(matches!(
            eval_str("sum(1)", root(&f)),
            Err(EvalError::ExpectedNodeSet { .. })
        ));
    }

    #[test]
    fn floor_and_ceiling() {
        let f = build();
        assert_eq!(number_of(eval_str("floor(1.9)", root(&f))), 1.0);
        assert_eq!(number_of(eval_str("floor(-1.1)", root(&f))), -2.0);
        assert_eq!(number_of(eval_str("ceiling(1.1)", root(&f))), 2.0);
        assert_eq!(number_of(eval_str("ceiling(-1.9)", root(&f))), -1.0);
    }

    // ---- round(): +Infinity tie-break and negative-zero special case --------

    #[test]
    fn round_ties_go_toward_positive_infinity() {
        let f = build();
        assert_eq!(number_of(eval_str("round(2.5)", root(&f))), 3.0);
        assert_eq!(
            number_of(eval_str("round(-2.5)", root(&f))),
            -2.0,
            "XPath rounds -2.5 toward +infinity, i.e. to -2, not -3"
        );
        assert_eq!(number_of(eval_str("round(0.5)", root(&f))), 1.0);
    }

    #[test]
    fn round_of_small_negative_is_negative_zero_not_positive_zero() {
        let f = build();
        let result = number_of(eval_str("round(-0.3)", root(&f)));
        assert_eq!(result, 0.0); // -0.0 == 0.0 is true in IEEE-754/Rust...
        assert!(
            result.is_sign_negative(),
            "round(-0.3) must be negative zero specifically, not positive zero"
        );

        let boundary = number_of(eval_str("round(-0.5)", root(&f)));
        assert!(
            boundary.is_sign_negative(),
            "round(-0.5) is the -0.5<=x<0 special case, so also negative zero"
        );
    }

    #[test]
    fn round_with_wrong_arity_is_an_error() {
        let f = build();
        assert!(matches!(
            eval_str("round()", root(&f)),
            Err(EvalError::ArgumentCount {
                function: "round",
                expected: 1,
                got: 0
            })
        ));
    }

    // ---- §4.1 id() ------------------------------------------------------

    #[test]
    fn id_with_wrong_arity_is_an_error() {
        let f = build();
        assert!(matches!(
            eval_str("id()", root(&f)),
            Err(EvalError::ArgumentCount {
                function: "id",
                expected: 1,
                got: 0
            })
        ));
    }

    /// A minimal parent-linked tree, purpose-built to test `id()` — the
    /// shared `document::fixture` tree never overrides `is_id_attribute()`
    /// (it's `false` everywhere by the trait default), so it can't exercise
    /// a real match. A standalone `Node` impl is used instead, following
    /// the same precedent as `QNameArena`/`LangArena`.
    #[derive(Debug)]
    enum IdEntry {
        Root {
            children: Vec<usize>,
        },
        Element {
            #[allow(dead_code)]
            parent: usize,
            attributes: Vec<usize>,
        },
        Attribute {
            owner: usize,
            local_name: &'static str,
            value: &'static str,
            is_id: bool,
        },
    }

    #[derive(Debug)]
    struct IdArena(Vec<IdEntry>);

    #[derive(Clone, Copy, Debug)]
    struct IdNode<'a> {
        arena: &'a IdArena,
        idx: usize,
    }

    impl<'a> PartialEq for IdNode<'a> {
        fn eq(&self, other: &Self) -> bool {
            std::ptr::eq(self.arena, other.arena) && self.idx == other.idx
        }
    }
    impl<'a> Eq for IdNode<'a> {}

    impl<'a> Node<'a> for IdNode<'a> {
        fn kind(self) -> NodeKind {
            match &self.arena.0[self.idx] {
                IdEntry::Root { .. } => NodeKind::Root,
                IdEntry::Element { .. } => NodeKind::Element,
                IdEntry::Attribute { .. } => NodeKind::Attribute,
            }
        }
        fn parent(self) -> Option<Self> {
            match &self.arena.0[self.idx] {
                IdEntry::Root { .. } => None,
                IdEntry::Element { parent, .. } => Some(IdNode {
                    arena: self.arena,
                    idx: *parent,
                }),
                IdEntry::Attribute { owner, .. } => Some(IdNode {
                    arena: self.arena,
                    idx: *owner,
                }),
            }
        }
        fn children(self) -> impl Iterator<Item = Self> + 'a {
            let arena = self.arena;
            let indices = match &arena.0[self.idx] {
                IdEntry::Root { children } => children.clone(),
                _ => Vec::new(),
            };
            indices.into_iter().map(move |i| IdNode { arena, idx: i })
        }
        fn attributes(self) -> impl Iterator<Item = Self> + 'a {
            let arena = self.arena;
            let indices = match &arena.0[self.idx] {
                IdEntry::Element { attributes, .. } => attributes.clone(),
                _ => Vec::new(),
            };
            indices.into_iter().map(move |i| IdNode { arena, idx: i })
        }
        fn namespaces(self) -> impl Iterator<Item = Self> + 'a {
            std::iter::empty()
        }
        fn expanded_name(self) -> Option<ExpandedName> {
            match &self.arena.0[self.idx] {
                IdEntry::Root { .. } => None,
                IdEntry::Element { .. } => Some(ExpandedName {
                    namespace_uri: None,
                    local_name: "el".to_string(),
                }),
                IdEntry::Attribute { local_name, .. } => Some(ExpandedName {
                    namespace_uri: None,
                    local_name: local_name.to_string(),
                }),
            }
        }
        fn string_value(self) -> String {
            match &self.arena.0[self.idx] {
                IdEntry::Attribute { value, .. } => value.to_string(),
                _ => String::new(),
            }
        }
        fn document_order(self, other: Self) -> std::cmp::Ordering {
            self.idx.cmp(&other.idx)
        }
        fn is_id_attribute(self) -> bool {
            matches!(
                &self.arena.0[self.idx],
                IdEntry::Attribute { is_id: true, .. }
            )
        }
    }

    /// `root(0) -> el1(1)[id="a", is_id] -> el2(3)[id="b", is_id] ->
    /// el3(5)[id="a", NOT is_id]` — `el3` is a same-named-attribute trap:
    /// its `id` attribute has the same local name and a colliding value as
    /// `el1`'s, but isn't marked `is_id_attribute`, so it must never match.
    fn id_arena() -> IdArena {
        IdArena(vec![
            IdEntry::Root {
                children: vec![1, 3, 5],
            },
            IdEntry::Element {
                parent: 0,
                attributes: vec![2],
            },
            IdEntry::Attribute {
                owner: 1,
                local_name: "id",
                value: "a",
                is_id: true,
            },
            IdEntry::Element {
                parent: 0,
                attributes: vec![4],
            },
            IdEntry::Attribute {
                owner: 3,
                local_name: "id",
                value: "b",
                is_id: true,
            },
            IdEntry::Element {
                parent: 0,
                attributes: vec![6],
            },
            IdEntry::Attribute {
                owner: 5,
                local_name: "id",
                value: "a",
                is_id: false,
            },
        ])
    }

    fn id_node(arena: &IdArena, idx: usize) -> IdNode<'_> {
        IdNode { arena, idx }
    }

    #[test]
    fn id_finds_the_element_with_the_matching_id_attribute() {
        let arena = id_arena();
        let ctx = EvaluationContext::new(id_node(&arena, 0));
        let expr = crate::parse("id('b')").unwrap();
        assert_eq!(
            crate::evaluate(&expr, &ctx),
            Ok(Value::NodeSet(vec![id_node(&arena, 3)]))
        );
    }

    #[test]
    fn id_does_not_match_a_same_named_attribute_that_is_not_marked_as_the_id() {
        let arena = id_arena();
        let ctx = EvaluationContext::new(id_node(&arena, 0));
        let expr = crate::parse("id('a')").unwrap();
        // el3 (idx 5) has an "id"-named attribute with the same value "a",
        // but `is_id_attribute() == false` — only el1 (idx 1) must match.
        assert_eq!(
            crate::evaluate(&expr, &ctx),
            Ok(Value::NodeSet(vec![id_node(&arena, 1)]))
        );
    }

    #[test]
    fn id_with_whitespace_separated_tokens_returns_matches_in_document_order() {
        let arena = id_arena();
        let ctx = EvaluationContext::new(id_node(&arena, 0));
        // Token order is "b a" (reversed), but the result must come back
        // in document order: el1 (idx 1) before el2 (idx 3).
        let expr = crate::parse("id('b a')").unwrap();
        assert_eq!(
            crate::evaluate(&expr, &ctx),
            Ok(Value::NodeSet(vec![id_node(&arena, 1), id_node(&arena, 3)]))
        );
    }

    #[test]
    fn id_returns_an_empty_node_set_when_nothing_matches() {
        let arena = id_arena();
        let ctx = EvaluationContext::new(id_node(&arena, 0));
        let expr = crate::parse("id('zzz')").unwrap();
        assert_eq!(crate::evaluate(&expr, &ctx), Ok(Value::NodeSet(Vec::new())));
    }

    #[test]
    fn id_accepts_a_node_set_argument_and_uses_each_nodes_string_value() {
        let arena = id_arena();
        // Context is el2 (idx 3) itself, so `@id` resolves to its own
        // "id"="b" attribute; `id(@id)` must then find el2 again.
        let ctx = EvaluationContext::new(id_node(&arena, 3));
        let expr = crate::parse("id(@id)").unwrap();
        assert_eq!(
            crate::evaluate(&expr, &ctx),
            Ok(Value::NodeSet(vec![id_node(&arena, 3)]))
        );
    }
}
