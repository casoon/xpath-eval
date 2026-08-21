//! Evaluates an `ast::Expr` against a `Document`/`Node<'a>` (§2-§3): the
//! value model and conversions live in `value.rs`; this module adds the
//! `EvaluationContext`, the operator semantics (§3.3/§3.4), and the full
//! location-path/predicate evaluation (§2.2-§2.4).
//!
//! `last()`/`position()`/`count()` are implemented as functions directly
//! here; the rest of the core function library (§4, `id()` excluded) is
//! implemented in `functions.rs` and dispatched to from
//! `evaluate_function` below (Phase 05).

use crate::ast::{
    AdditiveOp, Axis, EqualityOp, Expr, FilterExpr, FunctionCall, LocationPath, MultiplicativeOp,
    NodeTest, PathExpr, PrimaryExpr, QName, RelationalOp, Step,
};
use crate::axes;
use crate::document::{Node, NodeKind};
use crate::functions;
use crate::value::{Value, string_to_number};

/// A variable-binding lookup hook: given a variable's `QName`, returns its
/// bound value, if any.
pub type VariableLookup<'a, N> = dyn Fn(&QName) -> Option<Value<N>> + 'a;

/// A namespace-prefix resolution hook: given a name test's raw prefix
/// string, returns the namespace URI it's bound to, if any. `None` means
/// the prefix is unresolvable via this hook (distinct from the hook being
/// absent altogether, but both fall back the same way — see
/// `matches_node_test`'s doc comment).
pub type NamespaceLookup<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// The context an `Expr` is evaluated against (§2.2/§2.4): the context node,
/// its 1-based proximity position and the context size within whatever
/// node-set it was drawn from, a variable-binding lookup hook, and a
/// namespace-prefix resolution hook.
///
/// `None` in `variables` means "no variables are bound" — every
/// `PrimaryExpr::Variable` lookup then fails with `EvalError::UnboundVariable`.
/// Nothing in this phase binds variables yet (that is a `schematron-engine`
/// `<let>` concern), but `PrimaryExpr::Variable` still needs this real,
/// clearly-erroring path rather than a panic or a silently-wrong default.
///
/// `None` in `namespaces` means "no namespace bindings are declared" —
/// prefixed name tests then fall back to comparing the raw prefix string
/// directly against `namespace_uri` (see `matches_node_test`'s doc comment
/// for the full rationale).
///
/// `'ctx` is the node/document borrow lifetime (`N: Node<'ctx>`); `'hook` is
/// an independent lifetime for the `variables`/`namespaces` hook references.
/// The two are deliberately decoupled (see `plan/05b-decouple-hook-lifetime.md`):
/// a hook built locally within a single `evaluate()` call (e.g. a closure
/// over a temporary namespace-binding table) does not need to live as long
/// as the document itself, and there is no `'hook: 'ctx` (or reverse) bound
/// between them — nothing here ever stores the hook and the node together
/// in a way that would require one.
///
/// `_ctx` is a zero-sized marker: `'ctx` is only used in the `N: Node<'ctx>`
/// bound above, not in any field type on its own, and Rust requires every
/// declared lifetime parameter of a struct to appear in a field (`E0392`
/// otherwise) — this `PhantomData` satisfies that without adding any real
/// state.
#[derive(Clone, Copy)]
pub struct EvaluationContext<'ctx, 'hook, N: Node<'ctx>> {
    pub node: N,
    pub position: usize,
    pub size: usize,
    pub variables: Option<&'hook VariableLookup<'hook, N>>,
    pub namespaces: Option<&'hook NamespaceLookup<'hook>>,
    _ctx: std::marker::PhantomData<&'ctx ()>,
}

impl<'ctx, 'hook, N: Node<'ctx>> EvaluationContext<'ctx, 'hook, N> {
    /// A context whose node is both the sole member and the first member of
    /// its own (size-1) context node-set, with no variable bindings and no
    /// namespace context.
    pub fn new(node: N) -> Self {
        EvaluationContext {
            node,
            position: 1,
            size: 1,
            variables: None,
            namespaces: None,
            _ctx: std::marker::PhantomData,
        }
    }

    /// Same variable-binding and namespace-resolution hooks, different
    /// context node/position/size — used when descending into a predicate
    /// or a nested path step.
    fn with(&self, node: N, position: usize, size: usize) -> Self {
        EvaluationContext {
            node,
            position,
            size,
            variables: self.variables,
            namespaces: self.namespaces,
            _ctx: std::marker::PhantomData,
        }
    }
}

/// An evaluation-time error. Distinct from `ParseError` (`parser.rs`), which
/// covers syntax — this covers semantic failures once a well-formed `Expr`
/// is evaluated against a concrete context.
#[derive(Debug, Clone, PartialEq)]
pub enum EvalError {
    /// `PrimaryExpr::Function` named something other than one of the 26
    /// implemented core-library functions (`last`/`position`/`count`,
    /// Phase 04, plus the 23 more in `functions.rs`, Phase 05) — including
    /// `id()`, which is deliberately out of scope (see `functions.rs`'s
    /// module doc comment).
    UnknownFunction(QName),
    /// `PrimaryExpr::Variable` has no binding via the context's lookup hook.
    UnboundVariable(QName),
    /// A function called with the wrong number of arguments.
    ArgumentCount {
        function: &'static str,
        expected: usize,
        got: usize,
    },
    /// `count()`'s argument, or a `Union`/`FilterExpr`-predicate operand,
    /// did not evaluate to a node-set.
    ExpectedNodeSet { context: &'static str },
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::UnknownFunction(name) => {
                write!(f, "unknown function: {}", format_qname(name))
            }
            EvalError::UnboundVariable(name) => {
                write!(f, "unbound variable: ${}", format_qname(name))
            }
            EvalError::ArgumentCount {
                function,
                expected,
                got,
            } => write!(f, "{function}() expects {expected} argument(s), got {got}"),
            EvalError::ExpectedNodeSet { context } => {
                write!(f, "expected a node-set in {context}")
            }
        }
    }
}

impl std::error::Error for EvalError {}

fn format_qname(q: &QName) -> String {
    match &q.prefix {
        Some(p) => format!("{p}:{}", q.local),
        None => q.local.clone(),
    }
}

/// Evaluates `expr` against `ctx`, producing an XPath 1.0 value.
pub fn evaluate<'ctx, 'hook, N: Node<'ctx>>(
    expr: &Expr,
    ctx: &EvaluationContext<'ctx, 'hook, N>,
) -> Result<Value<N>, EvalError> {
    match expr {
        // §3.3: "Or"/"And" convert both operands to boolean. The spec does
        // not mandate an evaluation order, and expressions here have no
        // side effects other than errors — short-circuiting is a
        // deliberate, documented choice (not evaluating the right operand
        // can hide an error it would have raised).
        Expr::Or(l, r) => {
            if evaluate(l, ctx)?.to_boolean() {
                Ok(Value::Boolean(true))
            } else {
                Ok(Value::Boolean(evaluate(r, ctx)?.to_boolean()))
            }
        }
        Expr::And(l, r) => {
            if !evaluate(l, ctx)?.to_boolean() {
                Ok(Value::Boolean(false))
            } else {
                Ok(Value::Boolean(evaluate(r, ctx)?.to_boolean()))
            }
        }
        Expr::Equality(l, op, r) => {
            let lv = evaluate(l, ctx)?;
            let rv = evaluate(r, ctx)?;
            Ok(Value::Boolean(compare_equality(&lv, *op, &rv)))
        }
        Expr::Relational(l, op, r) => {
            let lv = evaluate(l, ctx)?;
            let rv = evaluate(r, ctx)?;
            Ok(Value::Boolean(compare_relational(&lv, *op, &rv)))
        }
        Expr::Additive(l, op, r) => {
            let lv = evaluate(l, ctx)?.to_number();
            let rv = evaluate(r, ctx)?.to_number();
            Ok(Value::Number(match op {
                AdditiveOp::Add => lv + rv,
                AdditiveOp::Sub => lv - rv,
            }))
        }
        Expr::Multiplicative(l, op, r) => {
            let lv = evaluate(l, ctx)?.to_number();
            let rv = evaluate(r, ctx)?.to_number();
            Ok(Value::Number(match op {
                MultiplicativeOp::Mul => lv * rv,
                // Rust's `f64` `/` and `%` already implement IEEE-754
                // division and remainder, which is exactly what XPath's
                // `div`/`mod` specify.
                MultiplicativeOp::Div => lv / rv,
                MultiplicativeOp::Mod => lv % rv,
            }))
        }
        Expr::Negate(e) => Ok(Value::Number(-evaluate(e, ctx)?.to_number())),
        Expr::Union(l, r) => {
            let lv = evaluate(l, ctx)?;
            let rv = evaluate(r, ctx)?;
            match (lv, rv) {
                (Value::NodeSet(mut a), Value::NodeSet(b)) => {
                    a.extend(b);
                    sort_dedup(&mut a);
                    Ok(Value::NodeSet(a))
                }
                _ => Err(EvalError::ExpectedNodeSet {
                    context: "union operand",
                }),
            }
        }
        Expr::Path(path) => evaluate_path(path, ctx),
    }
}

// ---- Equality / Relational (§3.3/§3.4) ---------------------------------

fn eq_result<T: PartialEq>(op: EqualityOp, l: T, r: T) -> bool {
    match op {
        EqualityOp::Eq => l == r,
        EqualityOp::Ne => l != r,
    }
}

fn rel_result(op: RelationalOp, l: f64, r: f64) -> bool {
    match op {
        RelationalOp::Lt => l < r,
        RelationalOp::Le => l <= r,
        RelationalOp::Gt => l > r,
        RelationalOp::Ge => l >= r,
    }
}

/// §3.4, node-set comparison rules for `=`/`!=` (all four cases quoted
/// verbatim in the plan), falling back to the "no node-set operand" rule
/// (boolean, else number, else string) otherwise.
fn compare_equality<'a, N: Node<'a>>(lhs: &Value<N>, op: EqualityOp, rhs: &Value<N>) -> bool {
    match (lhs, rhs) {
        (Value::NodeSet(a), Value::NodeSet(b)) => a.iter().copied().any(|na| {
            b.iter()
                .copied()
                .any(|nb| eq_result(op, na.string_value(), nb.string_value()))
        }),
        (Value::NodeSet(a), Value::Number(n)) | (Value::Number(n), Value::NodeSet(a)) => a
            .iter()
            .copied()
            .any(|na| eq_result(op, string_to_number(&na.string_value()), *n)),
        (Value::NodeSet(a), Value::String(s)) | (Value::String(s), Value::NodeSet(a)) => a
            .iter()
            .copied()
            .any(|na| eq_result(op, na.string_value(), s.clone())),
        (Value::NodeSet(a), Value::Boolean(b)) | (Value::Boolean(b), Value::NodeSet(a)) => {
            eq_result(op, !a.is_empty(), *b)
        }
        _ => {
            // §3.4, "neither object to be compared is a node-set": boolean
            // if either side is boolean, else number if either side is
            // number, else string.
            if matches!(lhs, Value::Boolean(_)) || matches!(rhs, Value::Boolean(_)) {
                eq_result(op, lhs.to_boolean(), rhs.to_boolean())
            } else if matches!(lhs, Value::Number(_)) || matches!(rhs, Value::Number(_)) {
                eq_result(op, lhs.to_number(), rhs.to_number())
            } else {
                eq_result(op, lhs.to_xpath_string(), rhs.to_xpath_string())
            }
        }
    }
}

/// §3.4, relational operators. When neither operand is a node-set, always
/// convert both to number (never string/boolean comparison). When a
/// node-set is involved, the same four node-set-vs-X cases as equality
/// apply, but — per the plan's clarification of the spec text — the
/// underlying per-node "comparison" is always a numeric one: node-set vs
/// node-set compares the two nodes' string-values *converted to numbers*
/// (not compared as strings), and node-set vs string/boolean likewise
/// converts the non-node-set side to a number before comparing, since a
/// relational operator's result must in the end come from a numeric
/// comparison.
fn compare_relational<'a, N: Node<'a>>(lhs: &Value<N>, op: RelationalOp, rhs: &Value<N>) -> bool {
    match (lhs, rhs) {
        (Value::NodeSet(a), Value::NodeSet(b)) => a.iter().copied().any(|na| {
            b.iter().copied().any(|nb| {
                rel_result(
                    op,
                    string_to_number(&na.string_value()),
                    string_to_number(&nb.string_value()),
                )
            })
        }),
        (Value::NodeSet(a), Value::Number(n)) => a
            .iter()
            .copied()
            .any(|na| rel_result(op, string_to_number(&na.string_value()), *n)),
        (Value::Number(n), Value::NodeSet(a)) => a
            .iter()
            .copied()
            .any(|na| rel_result(op, *n, string_to_number(&na.string_value()))),
        (Value::NodeSet(a), Value::String(s)) => {
            let sn = string_to_number(s);
            a.iter()
                .copied()
                .any(|na| rel_result(op, string_to_number(&na.string_value()), sn))
        }
        (Value::String(s), Value::NodeSet(a)) => {
            let sn = string_to_number(s);
            a.iter()
                .copied()
                .any(|na| rel_result(op, sn, string_to_number(&na.string_value())))
        }
        (Value::NodeSet(a), Value::Boolean(b)) => {
            rel_result(op, bool_to_number(!a.is_empty()), bool_to_number(*b))
        }
        (Value::Boolean(b), Value::NodeSet(a)) => {
            rel_result(op, bool_to_number(*b), bool_to_number(!a.is_empty()))
        }
        _ => rel_result(op, lhs.to_number(), rhs.to_number()),
    }
}

fn bool_to_number(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

// ---- Location paths / steps / predicates (§2.2-§2.4) -------------------

fn root_of<'a, N: Node<'a>>(n: N) -> N {
    let mut cur = n;
    while let Some(p) = cur.parent() {
        cur = p;
    }
    cur
}

fn sort_dedup<'a, N: Node<'a>>(nodes: &mut Vec<N>) {
    nodes.sort_by(|a, b| a.document_order(*b));
    nodes.dedup();
}

fn axis_nodes<'a, N: Node<'a>>(axis: Axis, n: N) -> Vec<N> {
    match axis {
        Axis::Ancestor => axes::ancestor(n).collect(),
        Axis::AncestorOrSelf => axes::ancestor_or_self(n).collect(),
        Axis::Attribute => axes::attribute(n).collect(),
        Axis::Child => axes::child(n).collect(),
        Axis::Descendant => axes::descendant(n).collect(),
        Axis::DescendantOrSelf => axes::descendant_or_self(n).collect(),
        Axis::Following => axes::following(n).collect(),
        Axis::FollowingSibling => axes::following_sibling(n).collect(),
        Axis::Namespace => axes::namespace(n).collect(),
        Axis::Parent => axes::parent(n).collect(),
        Axis::Preceding => axes::preceding(n).collect(),
        Axis::PrecedingSibling => axes::preceding_sibling(n).collect(),
        Axis::SelfAxis => axes::self_axis(n).collect(),
    }
}

/// The "principal node type" of an axis (§2.3): attribute nodes for the
/// attribute axis, namespace nodes for the namespace axis, elements for
/// every other axis. Only relevant to `NameTest` node tests (`*`,
/// `prefix:*`, `QName`) — node-type tests (`text()` etc.) match by kind
/// regardless of axis.
fn principal_node_kind(axis: Axis) -> NodeKind {
    match axis {
        Axis::Attribute => NodeKind::Attribute,
        Axis::Namespace => NodeKind::Namespace,
        _ => NodeKind::Element,
    }
}

/// Outcome of resolving a `NodeTest` prefix string via `resolve_prefix`.
enum PrefixResolution {
    /// A value to compare against `ExpandedName::namespace_uri` — either a
    /// hook-resolved URI, or (no hook present at all) the Phase-04
    /// fallback of treating the raw prefix string itself as that value.
    Value(String),
    /// A resolver hook *is* present but doesn't know this prefix. Per spec
    /// an unbound namespace prefix is invalid; conservatively, that can
    /// never match a real node's namespace URI — unlike the no-hook case,
    /// this must not fall back to comparing the raw prefix string, since
    /// that string was never claimed to be a URI.
    Unbound,
}

/// Resolves `prefix` to a namespace URI via `namespaces` (Phase 04a). See
/// `PrefixResolution` for the no-hook vs. hook-present-but-unresolvable
/// distinction.
fn resolve_prefix<'hook>(
    prefix: &str,
    namespaces: Option<&NamespaceLookup<'hook>>,
) -> PrefixResolution {
    match namespaces {
        None => PrefixResolution::Value(prefix.to_string()),
        Some(lookup) => match lookup(prefix) {
            Some(uri) => PrefixResolution::Value(uri),
            None => PrefixResolution::Unbound,
        },
    }
}

/// Note on namespace resolution: `NodeTest::QName`/`NamespaceWildcard` carry
/// the expression's raw `prefix` string. Before Phase 04a, this compared
/// that raw string directly against `ExpandedName::namespace_uri` — correct
/// only by coincidence, when a caller happened to use the URI itself as the
/// prefix. `namespaces` (threaded down from `EvaluationContext`, see
/// `resolve_prefix`) now resolves the prefix to its declared URI first,
/// when a resolver hook is present and knows the prefix; with no hook at
/// all, this falls back to the old raw-string comparison; with a hook
/// present that doesn't know the prefix, no node can match (see
/// `PrefixResolution::Unbound`).
fn matches_node_test<'ctx, 'hook, N: Node<'ctx>>(
    test: &NodeTest,
    axis: Axis,
    n: N,
    namespaces: Option<&NamespaceLookup<'hook>>,
) -> bool {
    match test {
        NodeTest::Node => true,
        NodeTest::Text => n.kind() == NodeKind::Text,
        NodeTest::Comment => n.kind() == NodeKind::Comment,
        NodeTest::ProcessingInstruction(target) => {
            n.kind() == NodeKind::ProcessingInstruction
                && match target {
                    None => true,
                    Some(t) => n.expanded_name().is_some_and(|e| e.local_name == *t),
                }
        }
        NodeTest::AnyName => n.kind() == principal_node_kind(axis) && n.expanded_name().is_some(),
        NodeTest::NamespaceWildcard(prefix) => {
            let uri = match resolve_prefix(prefix, namespaces) {
                PrefixResolution::Unbound => return false,
                PrefixResolution::Value(uri) => uri,
            };
            n.kind() == principal_node_kind(axis)
                && n.expanded_name()
                    .is_some_and(|e| e.namespace_uri.as_deref() == Some(uri.as_str()))
        }
        NodeTest::QName(qname) => {
            let uri = match &qname.prefix {
                None => None,
                Some(p) => match resolve_prefix(p, namespaces) {
                    PrefixResolution::Unbound => return false,
                    PrefixResolution::Value(uri) => Some(uri),
                },
            };
            n.kind() == principal_node_kind(axis)
                && n.expanded_name().is_some_and(|e| {
                    e.local_name == qname.local && e.namespace_uri.as_deref() == uri.as_deref()
                })
        }
    }
}

/// §2.4, verbatim: "A PredicateExpr is evaluated by evaluating the Expr and
/// converting the result to a boolean. If the result is a number, the
/// result will be converted to true if the number is equal to the context
/// position and will be converted to false otherwise; if the result is not
/// a number, then the result will be converted as if by a call to the
/// boolean() function."
fn predicate_truth<'a, N: Node<'a>>(val: &Value<N>, position: usize) -> bool {
    match val {
        Value::Number(n) => *n == position as f64,
        _ => val.to_boolean(),
    }
}

/// Applies each predicate in `predicates` to `nodes` **in sequence**: every
/// predicate's context size/position come from the node-set the *previous*
/// predicate filtered down to, not from the original, unfiltered input —
/// this is what makes `child::*[1][self::foo]` differ from
/// `child::*[self::foo][1]` (see `plan/04-evaluation-core.md`'s worked
/// example and the module-level test of the same shape).
fn apply_predicates<'ctx, 'hook, N: Node<'ctx>>(
    predicates: &[Expr],
    nodes: Vec<N>,
    ctx: &EvaluationContext<'ctx, 'hook, N>,
) -> Result<Vec<N>, EvalError> {
    let mut current = nodes;
    for pred in predicates {
        let size = current.len();
        let mut next = Vec::with_capacity(current.len());
        for (i, &n) in current.iter().enumerate() {
            let position = i + 1;
            let pred_ctx = ctx.with(n, position, size);
            let val = evaluate(pred, &pred_ctx)?;
            if predicate_truth(&val, position) {
                next.push(n);
            }
        }
        current = next;
    }
    Ok(current)
}

/// Evaluates one `Step` against each of `context_nodes` individually — the
/// axis is walked and node-test-filtered per context node (so per-node
/// proximity positions/predicates are computed against that node's own
/// axis order), and only the union of all those per-node results is sorted
/// into document order and deduplicated at the end.
fn evaluate_step<'ctx, 'hook, N: Node<'ctx>>(
    step: &Step,
    context_nodes: &[N],
    ctx: &EvaluationContext<'ctx, 'hook, N>,
) -> Result<Vec<N>, EvalError> {
    let mut result = Vec::new();
    for &n in context_nodes {
        let candidates: Vec<N> = axis_nodes(step.axis, n)
            .into_iter()
            .filter(|&c| matches_node_test(&step.node_test, step.axis, c, ctx.namespaces))
            .collect();
        let filtered = apply_predicates(&step.predicates, candidates, ctx)?;
        result.extend(filtered);
    }
    sort_dedup(&mut result);
    Ok(result)
}

fn evaluate_location_steps<'ctx, 'hook, N: Node<'ctx>>(
    steps: &[Step],
    mut nodes: Vec<N>,
    ctx: &EvaluationContext<'ctx, 'hook, N>,
) -> Result<Vec<N>, EvalError> {
    for step in steps {
        nodes = evaluate_step(step, &nodes, ctx)?;
    }
    Ok(nodes)
}

fn evaluate_location_path<'ctx, 'hook, N: Node<'ctx>>(
    loc: &LocationPath,
    start: Vec<N>,
    ctx: &EvaluationContext<'ctx, 'hook, N>,
) -> Result<Vec<N>, EvalError> {
    let start = if loc.is_absolute {
        start.first().map(|&n| vec![root_of(n)]).unwrap_or_default()
    } else {
        start
    };
    evaluate_location_steps(&loc.steps, start, ctx)
}

fn evaluate_path<'ctx, 'hook, N: Node<'ctx>>(
    path: &PathExpr,
    ctx: &EvaluationContext<'ctx, 'hook, N>,
) -> Result<Value<N>, EvalError> {
    match path {
        PathExpr::Location(loc) => {
            let result = evaluate_location_path(loc, vec![ctx.node], ctx)?;
            Ok(Value::NodeSet(result))
        }
        PathExpr::Filter(filter) => evaluate_filter_expr(filter, ctx),
        PathExpr::FilterLocation(filter, loc) => {
            let filter_val = evaluate_filter_expr(filter, ctx)?;
            let start = match filter_val {
                Value::NodeSet(nodes) => nodes,
                _ => {
                    return Err(EvalError::ExpectedNodeSet {
                        context: "path expression left of '/'",
                    });
                }
            };
            let result = evaluate_location_path(loc, start, ctx)?;
            Ok(Value::NodeSet(result))
        }
    }
}

fn evaluate_filter_expr<'ctx, 'hook, N: Node<'ctx>>(
    filter: &FilterExpr,
    ctx: &EvaluationContext<'ctx, 'hook, N>,
) -> Result<Value<N>, EvalError> {
    let primary_val = evaluate_primary(&filter.primary, ctx)?;
    if filter.predicates.is_empty() {
        return Ok(primary_val);
    }
    match primary_val {
        Value::NodeSet(mut nodes) => {
            // `nodes` may come from a variable binding (`PrimaryExpr::Variable`)
            // in caller-supplied, arbitrary order — unlike a location step's
            // result, it isn't guaranteed to already be in document order.
            // Predicate proximity position must be document-order-based
            // (§2.4), so sort (and dedup, for the same reason `evaluate_step`
            // does) before applying predicates.
            sort_dedup(&mut nodes);
            let filtered = apply_predicates(&filter.predicates, nodes, ctx)?;
            Ok(Value::NodeSet(filtered))
        }
        _ => Err(EvalError::ExpectedNodeSet {
            context: "filter expression predicate",
        }),
    }
}

fn evaluate_primary<'ctx, 'hook, N: Node<'ctx>>(
    primary: &PrimaryExpr,
    ctx: &EvaluationContext<'ctx, 'hook, N>,
) -> Result<Value<N>, EvalError> {
    match primary {
        PrimaryExpr::Variable(qname) => {
            let bound = ctx.variables.and_then(|lookup| lookup(qname));
            bound.ok_or_else(|| EvalError::UnboundVariable(qname.clone()))
        }
        PrimaryExpr::Parenthesized(inner) => evaluate(inner, ctx),
        PrimaryExpr::Literal(s) => Ok(Value::String(s.clone())),
        PrimaryExpr::Number(n) => Ok(Value::Number(*n)),
        PrimaryExpr::Function(call) => evaluate_function(call, ctx),
    }
}

fn check_arity(function: &'static str, args: &[Expr], expected: usize) -> Result<(), EvalError> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(EvalError::ArgumentCount {
            function,
            expected,
            got: args.len(),
        })
    }
}

/// `last()`/`position()`/`count()` are implemented here (Phase 04); every
/// other core-library function (Phase 05, `id()` excluded) is dispatched to
/// `functions::dispatch`, which itself falls back to
/// `EvalError::UnknownFunction` for any name it does not implement either —
/// deliberately: a generic fallback would hide the difference between "not
/// yet implemented" and "genuinely does not exist".
fn evaluate_function<'ctx, 'hook, N: Node<'ctx>>(
    call: &FunctionCall,
    ctx: &EvaluationContext<'ctx, 'hook, N>,
) -> Result<Value<N>, EvalError> {
    if call.name.prefix.is_none() {
        match call.name.local.as_str() {
            "last" => {
                check_arity("last", &call.args, 0)?;
                return Ok(Value::Number(ctx.size as f64));
            }
            "position" => {
                check_arity("position", &call.args, 0)?;
                return Ok(Value::Number(ctx.position as f64));
            }
            "count" => {
                check_arity("count", &call.args, 1)?;
                return match evaluate(&call.args[0], ctx)? {
                    Value::NodeSet(nodes) => Ok(Value::Number(nodes.len() as f64)),
                    _ => Err(EvalError::ExpectedNodeSet {
                        context: "count() argument",
                    }),
                };
            }
            _ => {}
        }
    }
    functions::dispatch(call, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::fixture::{Fixture, TestNode, build};

    fn eval_str<'a>(src: &str, node: TestNode<'a>) -> Result<Value<TestNode<'a>>, EvalError> {
        let expr = crate::parse(src).expect("test expression must parse");
        let ctx = EvaluationContext::new(node);
        evaluate(&expr, &ctx)
    }

    fn node_set<'a>(v: Result<Value<TestNode<'a>>, EvalError>) -> Vec<TestNode<'a>> {
        match v.expect("expected Ok") {
            Value::NodeSet(nodes) => nodes,
            other => panic!("expected a node-set, got {other:?}"),
        }
    }

    fn root(f: &Fixture) -> TestNode<'_> {
        f.doc.node(f.html)
    }

    // ---- Or / And (short-circuit) --------------------------------------

    #[test]
    fn or_short_circuits_when_left_is_true() {
        let f = build();
        assert_eq!(
            eval_str("(1 = 1) or unknownfn()", root(&f)),
            Ok(Value::Boolean(true))
        );
    }

    #[test]
    fn or_evaluates_right_when_left_is_false() {
        let f = build();
        assert!(matches!(
            eval_str("(1 = 0) or unknownfn()", root(&f)),
            Err(EvalError::UnknownFunction(_))
        ));
    }

    #[test]
    fn and_short_circuits_when_left_is_false() {
        let f = build();
        assert_eq!(
            eval_str("(1 = 0) and unknownfn()", root(&f)),
            Ok(Value::Boolean(false))
        );
    }

    #[test]
    fn and_evaluates_right_when_left_is_true() {
        let f = build();
        assert!(matches!(
            eval_str("(1 = 1) and unknownfn()", root(&f)),
            Err(EvalError::UnknownFunction(_))
        ));
    }

    // ---- Equality: all four node-set comparison cases -------------------

    #[test]
    fn equality_node_set_vs_node_set() {
        let f = build();
        assert_eq!(
            eval_str(
                "/html/body/span/@data-x = /html/body/span/@data-x",
                root(&f)
            ),
            Ok(Value::Boolean(true))
        );
        assert_eq!(
            eval_str("/html/body/span/@data-x = /html/@id", root(&f)),
            Ok(Value::Boolean(false))
        );
        assert_eq!(
            eval_str("/html/body/span/@data-x != /html/@id", root(&f)),
            Ok(Value::Boolean(true))
        );
    }

    #[test]
    fn equality_node_set_vs_number() {
        let f = build();
        assert_eq!(
            eval_str("/html/body/span/@data-x = 1", root(&f)),
            Ok(Value::Boolean(true))
        );
        assert_eq!(
            eval_str("/html/body/span/@data-x = 2", root(&f)),
            Ok(Value::Boolean(false))
        );
        assert_eq!(
            eval_str("/html/body/span/@data-x != 2", root(&f)),
            Ok(Value::Boolean(true))
        );
    }

    #[test]
    fn equality_node_set_vs_string() {
        let f = build();
        assert_eq!(
            eval_str("/html/body/span/@data-x = '1'", root(&f)),
            Ok(Value::Boolean(true))
        );
        assert_eq!(
            eval_str("/html/@id = 'nope'", root(&f)),
            Ok(Value::Boolean(false))
        );
    }

    #[test]
    fn equality_node_set_vs_boolean() {
        let f = build();
        // Non-empty node-set converts to boolean true.
        assert_eq!(
            eval_str("/html/body/span/@data-x = (1 = 1)", root(&f)),
            Ok(Value::Boolean(true))
        );
        // Empty node-set converts to boolean false.
        assert_eq!(
            eval_str("/html/body/nonexistent = (1 = 1)", root(&f)),
            Ok(Value::Boolean(false))
        );
    }

    // ---- Relational operators with a node-set operand -------------------

    #[test]
    fn relational_with_node_set_operand() {
        let f = build();
        assert_eq!(
            eval_str("/html/body/span/@data-x > 0", root(&f)),
            Ok(Value::Boolean(true))
        );
        assert_eq!(
            eval_str("/html/body/span/@data-x < 0", root(&f)),
            Ok(Value::Boolean(false))
        );
        // node-set vs string: the string is converted to a number too.
        assert_eq!(
            eval_str("/html/body/span/@data-x > '0'", root(&f)),
            Ok(Value::Boolean(true))
        );
        // node-set vs boolean.
        assert_eq!(
            eval_str("/html/body/span/@data-x > (1 = 0)", root(&f)),
            Ok(Value::Boolean(true))
        );
    }

    // ---- Additive / Multiplicative / Negate ------------------------------

    #[test]
    fn arithmetic_operators() {
        let f = build();
        assert_eq!(eval_str("1 + 2", root(&f)), Ok(Value::Number(3.0)));
        assert_eq!(eval_str("1 - 2", root(&f)), Ok(Value::Number(-1.0)));
        assert_eq!(eval_str("2 * 3", root(&f)), Ok(Value::Number(6.0)));
        assert_eq!(eval_str("7 div 2", root(&f)), Ok(Value::Number(3.5)));
        assert_eq!(eval_str("7 mod 3", root(&f)), Ok(Value::Number(1.0)));
        assert_eq!(eval_str("-1 + 2", root(&f)), Ok(Value::Number(1.0)));
    }

    // ---- Union with dedup -------------------------------------------------

    #[test]
    fn union_deduplicates_overlapping_node_sets() {
        let f = build();
        let nodes = node_set(eval_str(
            "/html/body/child::node() | /html/body/child::*",
            root(&f),
        ));
        assert_eq!(
            nodes,
            vec![
                f.doc.node(f.text0),
                f.doc.node(f.span),
                f.doc.node(f.comment),
                f.doc.node(f.pi),
                f.doc.node(f.text2),
            ],
            "span (present in both operands) must appear exactly once, in document order"
        );
    }

    // ---- Multi-step paths with predicates --------------------------------

    #[test]
    fn multi_step_path_with_numeric_predicate() {
        let f = build();
        let nodes = node_set(eval_str("/html/body/child::node()[3]", root(&f)));
        assert_eq!(nodes, vec![f.doc.node(f.comment)]);
    }

    #[test]
    fn multi_step_mixed_axis_path_with_predicate() {
        let f = build();
        // descendant-or-self (via "//") + child::text()[2] — the classic
        // XPath gotcha: `[2]` is *not* "the 2nd text node in the whole
        // subtree". A step's predicates apply once per context node
        // contributed by the previous step, each with its own local
        // position/size, and the results are unioned — exactly like the
        // well-known `//para[1]` example (which selects the first `para`
        // *child of each element that has one*, not the first `para` in
        // the document). Here, `descendant-or-self::node()` contributes
        // html/body/text0/span/text1/comment/pi/text2 as context nodes;
        // only `body` has 2+ text children ([text0, text2]), so `[2]`
        // picks `body`'s 2nd text child (text2) — `span`'s single text
        // child (text1) has no position 2 and contributes nothing.
        let nodes = node_set(eval_str("/html//text()[2]", root(&f)));
        assert_eq!(nodes, vec![f.doc.node(f.text2)]);
    }

    // ---- Sequential predicate filtering (the plan's flagged risk) --------

    #[test]
    fn predicate_order_changes_the_result() {
        let f = build();
        // body's children in document order: text0, span, comment, pi, text2
        // — span is the 2nd child.
        let position_then_test = node_set(eval_str(
            "/html/body/child::node()[2][self::span]",
            root(&f),
        ));
        assert_eq!(
            position_then_test,
            vec![f.doc.node(f.span)],
            "[2] keeps the 2nd child (span); [self::span] then confirms it against itself"
        );

        let test_then_position = node_set(eval_str(
            "/html/body/child::node()[self::span][2]",
            root(&f),
        ));
        assert_eq!(
            test_then_position,
            Vec::<TestNode>::new(),
            "[self::span] first narrows to just [span] (size 1); [2] then has nothing at position 2"
        );
    }

    // ---- last()/position()/count() ---------------------------------------

    #[test]
    fn last_position_count_in_predicates() {
        let f = build();
        assert_eq!(
            node_set(eval_str("/html/body/child::node()[last()]", root(&f))),
            vec![f.doc.node(f.text2)]
        );
        assert_eq!(
            node_set(eval_str(
                "/html/body/child::node()[position() > 1]",
                root(&f)
            )),
            vec![
                f.doc.node(f.span),
                f.doc.node(f.comment),
                f.doc.node(f.pi),
                f.doc.node(f.text2),
            ]
        );
        assert_eq!(
            eval_str("count(/html/body/child::node())", root(&f)),
            Ok(Value::Number(5.0))
        );
    }

    // ---- §2.4 predicate coercion: numeric vs. boolean predicate ----------

    #[test]
    fn numeric_predicate_selects_by_position() {
        let f = build();
        let nodes = node_set(eval_str("/html/body/child::node()[3]", root(&f)));
        assert_eq!(nodes, vec![f.doc.node(f.comment)]);
    }

    #[test]
    fn non_numeric_predicate_uses_boolean_coercion() {
        let f = build();
        // "span" as a predicate means "does this candidate have a child
        // named span?" — none of body's children (text/comment/pi nodes,
        // or span itself, whose own child is a text node) do.
        let nodes = node_set(eval_str("/html/body/child::node()[span]", root(&f)));
        assert_eq!(nodes, Vec::<TestNode>::new());
    }

    // ---- Errors -----------------------------------------------------------

    #[test]
    fn unknown_function_is_an_error() {
        let f = build();
        assert!(matches!(
            eval_str("foo()", root(&f)),
            Err(EvalError::UnknownFunction(_))
        ));
    }

    #[test]
    fn unbound_variable_is_an_error() {
        let f = build();
        assert!(matches!(
            eval_str("$missing", root(&f)),
            Err(EvalError::UnboundVariable(_))
        ));
    }

    #[test]
    fn wrong_arity_is_an_error() {
        let f = build();
        assert!(matches!(
            eval_str("last(1)", root(&f)),
            Err(EvalError::ArgumentCount {
                function: "last",
                expected: 0,
                got: 1
            })
        ));
        assert!(matches!(
            eval_str("count()", root(&f)),
            Err(EvalError::ArgumentCount {
                function: "count",
                expected: 1,
                got: 0
            })
        ));
    }

    #[test]
    fn count_of_non_node_set_is_an_error() {
        let f = build();
        assert!(matches!(
            eval_str("count(1)", root(&f)),
            Err(EvalError::ExpectedNodeSet { .. })
        ));
    }

    // ---- Variable-binding hook ---------------------------------------------

    #[test]
    fn bound_variable_resolves_via_the_lookup_hook() {
        let f = build();
        let lookup = |q: &QName| -> Option<Value<TestNode<'_>>> {
            if q.prefix.is_none() && q.local == "x" {
                Some(Value::Number(42.0))
            } else {
                None
            }
        };
        let ctx = EvaluationContext {
            node: root(&f),
            position: 1,
            size: 1,
            variables: Some(&lookup),
            namespaces: None,
            _ctx: std::marker::PhantomData,
        };
        let expr = crate::parse("$x + 1").unwrap();
        assert_eq!(evaluate(&expr, &ctx), Ok(Value::Number(43.0)));
    }

    #[test]
    fn filter_expr_sorts_a_variable_supplied_node_set_into_document_order_before_predicates() {
        let f = build();
        let span = f.doc.node(f.span);
        let pi = f.doc.node(f.pi);
        // Deliberately unsorted: `pi` comes after `span` in document order,
        // but is listed first here. `$nodes[1]` must still pick the
        // document-order-first node (`span`), not the first `Vec` element —
        // otherwise it depends on the caller's arbitrary `Vec` order.
        let lookup = |q: &QName| -> Option<Value<TestNode<'_>>> {
            if q.prefix.is_none() && q.local == "nodes" {
                Some(Value::NodeSet(vec![pi, span]))
            } else {
                None
            }
        };
        let ctx = EvaluationContext {
            node: root(&f),
            position: 1,
            size: 1,
            variables: Some(&lookup),
            namespaces: None,
            _ctx: std::marker::PhantomData,
        };
        let expr = crate::parse("$nodes[1]").unwrap();
        assert_eq!(evaluate(&expr, &ctx), Ok(Value::NodeSet(vec![span])));
    }

    // ---- Namespace-resolution hook (Phase 04a) -----------------------------
    //
    // `matches_node_test` is exercised directly here (not through
    // `evaluate`/`parse`) because it needs a node whose `expanded_name()`
    // carries a real, non-`None` `namespace_uri` — the shared
    // `document::fixture` tree never populates one (see the pre-existing
    // doc comment on `matches_node_test`), so a minimal standalone `Node`
    // impl is used instead of extending that fixture (out of scope here,
    // touching `document.rs`).

    /// A single-node `Node` impl whose only purpose is carrying a chosen
    /// `namespace_uri`/`local_name` pair for the namespace-resolution tests
    /// below.
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct NsTestNode {
        namespace_uri: Option<&'static str>,
        local_name: &'static str,
    }

    impl<'a> Node<'a> for NsTestNode {
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

        fn expanded_name(self) -> Option<crate::document::ExpandedName> {
            Some(crate::document::ExpandedName {
                namespace_uri: self.namespace_uri.map(str::to_string),
                local_name: self.local_name.to_string(),
            })
        }

        fn string_value(self) -> String {
            String::new()
        }

        fn document_order(self, _other: Self) -> std::cmp::Ordering {
            std::cmp::Ordering::Equal
        }
    }

    fn ns_node() -> NsTestNode {
        NsTestNode {
            namespace_uri: Some("http://example/ns"),
            local_name: "rect",
        }
    }

    fn qname_test() -> NodeTest {
        NodeTest::QName(QName {
            prefix: Some("p".to_string()),
            local: "rect".to_string(),
        })
    }

    #[test]
    fn qname_test_matches_when_hook_resolves_prefix_to_the_real_uri() {
        let lookup =
            |p: &str| -> Option<String> { (p == "p").then(|| "http://example/ns".to_string()) };
        assert!(matches_node_test(
            &qname_test(),
            Axis::Child,
            ns_node(),
            Some(&lookup)
        ));
    }

    #[test]
    fn qname_test_does_not_match_when_hook_does_not_know_the_prefix() {
        // The hook is present but returns `None` for "p" — that prefix is
        // unbound, so no node can match (`PrefixResolution::Unbound`),
        // regardless of what the raw prefix string happens to look like.
        let lookup = |_: &str| -> Option<String> { None };
        assert!(!matches_node_test(
            &qname_test(),
            Axis::Child,
            ns_node(),
            Some(&lookup)
        ));
    }

    #[test]
    fn qname_test_does_not_match_when_hook_present_but_unresolvable_even_if_raw_prefix_equals_the_uri()
     {
        // Regression for the todo.md item: with a resolver hook present,
        // an unresolvable prefix must NOT fall back to comparing the raw
        // prefix string against the namespace URI — not even when that
        // raw string happens to equal the real URI.
        let mut node = ns_node();
        node.namespace_uri = Some("p");
        let lookup = |_: &str| -> Option<String> { None };
        assert!(!matches_node_test(
            &qname_test(),
            Axis::Child,
            node,
            Some(&lookup)
        ));
    }

    #[test]
    fn qname_test_does_not_match_with_no_hook_at_all() {
        // No hook at all (`namespaces: None`) — same fallback path as
        // above: raw prefix string "p" compared directly against the real
        // URI, not equal, so this must NOT match.
        assert!(!matches_node_test(
            &qname_test(),
            Axis::Child,
            ns_node(),
            None
        ));
    }

    #[test]
    fn qname_test_matches_with_no_hook_at_all_when_raw_prefix_equals_the_uri() {
        // No hook at all — Phase-04 fallback: the raw prefix string itself
        // is compared directly against the namespace URI, so this DOES
        // match when a caller happens to use the URI as the "prefix".
        // This is the fallback path `qname_test_does_not_match_when_hook_present_but_unresolvable_*`
        // above must NOT take once a hook is present.
        let mut node = ns_node();
        node.namespace_uri = Some("p");
        assert!(matches_node_test(&qname_test(), Axis::Child, node, None));
    }

    #[test]
    fn namespace_wildcard_matches_when_hook_resolves_prefix_to_the_real_uri() {
        let lookup =
            |p: &str| -> Option<String> { (p == "p").then(|| "http://example/ns".to_string()) };
        let test = NodeTest::NamespaceWildcard("p".to_string());
        assert!(matches_node_test(
            &test,
            Axis::Child,
            ns_node(),
            Some(&lookup)
        ));
    }

    #[test]
    fn namespace_wildcard_does_not_match_when_hook_does_not_know_the_prefix() {
        let lookup = |_: &str| -> Option<String> { None };
        let test = NodeTest::NamespaceWildcard("p".to_string());
        assert!(!matches_node_test(
            &test,
            Axis::Child,
            ns_node(),
            Some(&lookup)
        ));
    }

    #[test]
    fn namespace_wildcard_does_not_match_when_hook_present_but_unresolvable_even_if_raw_prefix_equals_the_uri()
     {
        let mut node = ns_node();
        node.namespace_uri = Some("p");
        let lookup = |_: &str| -> Option<String> { None };
        let test = NodeTest::NamespaceWildcard("p".to_string());
        assert!(!matches_node_test(&test, Axis::Child, node, Some(&lookup)));
    }

    #[test]
    fn namespace_wildcard_does_not_match_with_no_hook_at_all() {
        let test = NodeTest::NamespaceWildcard("p".to_string());
        assert!(!matches_node_test(&test, Axis::Child, ns_node(), None));
    }

    // ---- Phase 05b regression: hook lifetime decoupled from doc lifetime --
    //
    // A minimal parent/child-linked tree, purpose-built (like `NsTestNode`
    // above) because `document::fixture`'s shared tree never sets a real
    // namespace URI on an element. Unlike `NsTestNode`, this one has actual
    // parent/child links so a real `evaluate()` call — parse, axis walk,
    // node-test filtering, all of it — can be exercised end-to-end, not
    // just `matches_node_test` in isolation.
    #[derive(Debug)]
    enum HookLifetimeEntry {
        Root {
            children: Vec<usize>,
        },
        Element {
            #[allow(dead_code)]
            parent: usize,
            namespace_uri: Option<&'static str>,
            local_name: &'static str,
        },
    }

    #[derive(Debug)]
    struct HookLifetimeArena(Vec<HookLifetimeEntry>);

    #[derive(Clone, Copy, Debug)]
    struct HookLifetimeNode<'a> {
        arena: &'a HookLifetimeArena,
        idx: usize,
    }

    impl<'a> PartialEq for HookLifetimeNode<'a> {
        fn eq(&self, other: &Self) -> bool {
            std::ptr::eq(self.arena, other.arena) && self.idx == other.idx
        }
    }
    impl<'a> Eq for HookLifetimeNode<'a> {}

    impl<'a> Node<'a> for HookLifetimeNode<'a> {
        fn kind(self) -> NodeKind {
            match &self.arena.0[self.idx] {
                HookLifetimeEntry::Root { .. } => NodeKind::Root,
                HookLifetimeEntry::Element { .. } => NodeKind::Element,
            }
        }
        fn parent(self) -> Option<Self> {
            match &self.arena.0[self.idx] {
                HookLifetimeEntry::Root { .. } => None,
                HookLifetimeEntry::Element { parent, .. } => Some(HookLifetimeNode {
                    arena: self.arena,
                    idx: *parent,
                }),
            }
        }
        fn children(self) -> impl Iterator<Item = Self> + 'a {
            let arena = self.arena;
            let indices = match &arena.0[self.idx] {
                HookLifetimeEntry::Root { children } => children.clone(),
                HookLifetimeEntry::Element { .. } => Vec::new(),
            };
            indices
                .into_iter()
                .map(move |i| HookLifetimeNode { arena, idx: i })
        }
        fn attributes(self) -> impl Iterator<Item = Self> + 'a {
            std::iter::empty()
        }
        fn namespaces(self) -> impl Iterator<Item = Self> + 'a {
            std::iter::empty()
        }
        fn expanded_name(self) -> Option<crate::document::ExpandedName> {
            match &self.arena.0[self.idx] {
                HookLifetimeEntry::Root { .. } => None,
                HookLifetimeEntry::Element {
                    namespace_uri,
                    local_name,
                    ..
                } => Some(crate::document::ExpandedName {
                    namespace_uri: namespace_uri.map(str::to_string),
                    local_name: local_name.to_string(),
                }),
            }
        }
        fn string_value(self) -> String {
            String::new()
        }
        fn document_order(self, other: Self) -> std::cmp::Ordering {
            self.idx.cmp(&other.idx)
        }
    }

    #[test]
    fn evaluate_result_with_locally_scoped_namespace_hook_outlives_the_hook() {
        // The document: lives for this whole test function — clearly
        // longer than the inner block below, exactly like a caller's
        // document living as long as its own function's `'a`.
        let arena = HookLifetimeArena(vec![
            HookLifetimeEntry::Root {
                children: vec![1, 2],
            },
            HookLifetimeEntry::Element {
                parent: 0,
                namespace_uri: Some("http://example/ns"),
                local_name: "rect",
            },
            HookLifetimeEntry::Element {
                parent: 0,
                namespace_uri: None,
                local_name: "other",
            },
        ]);
        let root_node = HookLifetimeNode {
            arena: &arena,
            idx: 0,
        };

        // This inner block is the point of the test (see
        // `plan/05b-decouple-hook-lifetime.md`): the namespace-resolution
        // hook, and the `EvaluationContext` borrowing it via `'hook`, are
        // both built *and dropped* here — modeling a `schematron-engine`
        // caller that builds its namespace hook locally from a `Schema`'s
        // `<ns>` bindings inside a function body. Before Phase 05b,
        // `EvaluationContext`'s single lifetime forced this hook to live
        // exactly as long as `root_node`'s document borrow, so returning
        // `nodes` (computed via the hook, but only actually borrowing the
        // outer, longer-lived `arena`) out of this block could never
        // compile (rustc E0515). With `'hook` decoupled from `'ctx`, it
        // does.
        let nodes = {
            let bound_uri = "http://example/ns".to_string();
            let namespace_hook = move |prefix: &str| -> Option<String> {
                (prefix == "p").then(|| bound_uri.clone())
            };
            let ctx = EvaluationContext {
                node: root_node,
                position: 1,
                size: 1,
                variables: None,
                namespaces: Some(&namespace_hook),
                _ctx: std::marker::PhantomData,
            };
            let expr = crate::parse("p:rect").expect("test expression must parse");
            match evaluate(&expr, &ctx).expect("expected Ok") {
                Value::NodeSet(nodes) => nodes,
                other => panic!("expected a node-set, got {other:?}"),
            }
        };

        // Used *after* the inner scope (and the hook it contained) has
        // ended — the actual assertion that the fix works, not just that
        // it compiles.
        assert_eq!(nodes.len(), 1);
        assert_eq!(
            nodes[0],
            HookLifetimeNode {
                arena: &arena,
                idx: 1
            }
        );
    }
}
