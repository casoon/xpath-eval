//! The 13 XPath 1.0 axes (§2.2), derived generically from `Node`'s minimal
//! navigation primitives (`parent()`/`children()`/`attributes()`/
//! `namespaces()`) — see `plan/03-document-trait.md` for the verbatim spec
//! quotes each function below is checked against.
//!
//! Axes are free functions, not `Node` trait methods (see the plan's
//! module-split rationale). Reverse axes (`ancestor`, `ancestor-or-self`,
//! `preceding`, `preceding-sibling`) return their results in *reverse*
//! document order (nearest node first), per §2.4 proximity position.
//!
//! Implementation note: several axes (`following`, `preceding`) are built
//! on top of a full document-order listing of the tree, which is O(n) per
//! call — acceptable for this phase (correctness of a test-only reference
//! implementation), not a performance-tuned production axis evaluator.

use crate::document::{Node, NodeKind};

fn is_attribute_or_namespace(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::Attribute | NodeKind::Namespace)
}

/// The nearest-first chain of ancestors (`parent()`, `parent().parent()`,
/// ...), i.e. already in reverse document order.
fn ancestor_vec<'a, N: Node<'a>>(n: N) -> Vec<N> {
    let mut out = Vec::new();
    let mut cur = n.parent();
    while let Some(p) = cur {
        out.push(p);
        cur = p.parent();
    }
    out
}

/// All descendants of `n`, in document order.
fn descendant_vec<'a, N: Node<'a>>(n: N) -> Vec<N> {
    let mut out = Vec::new();
    for c in n.children() {
        out.push(c);
        out.extend(descendant_vec(c));
    }
    out
}

/// `n` followed by all its descendants, in document order.
fn descendant_or_self_vec<'a, N: Node<'a>>(n: N) -> Vec<N> {
    let mut out = vec![n];
    out.extend(descendant_vec(n));
    out
}

/// The root of the document `n` belongs to (walks `parent()` to the top).
pub(crate) fn root_of<'a, N: Node<'a>>(n: N) -> N {
    let mut cur = n;
    while let Some(p) = cur.parent() {
        cur = p;
    }
    cur
}

/// `child` axis: "contains the children of the context node".
pub fn child<'a, N: Node<'a>>(n: N) -> impl Iterator<Item = N> + 'a {
    n.children()
}

/// `descendant` axis: "contains the descendants of the context node; ...
/// thus the descendant axis never contains attribute or namespace nodes".
pub fn descendant<'a, N: Node<'a>>(n: N) -> impl Iterator<Item = N> + 'a {
    descendant_vec(n).into_iter()
}

/// `descendant-or-self` axis: "contains the context node and the
/// descendants of the context node".
pub fn descendant_or_self<'a, N: Node<'a>>(n: N) -> impl Iterator<Item = N> + 'a {
    descendant_or_self_vec(n).into_iter()
}

/// `parent` axis: "contains the parent of the context node, if there is
/// one".
pub fn parent<'a, N: Node<'a>>(n: N) -> impl Iterator<Item = N> + 'a {
    n.parent().into_iter()
}

/// `ancestor` axis (reverse): "contains the ancestors of the context node;
/// ... the parent of the context node and the parent's parent and so on;
/// thus, the ancestor axis will always include the root node, unless the
/// context node is the root node". Results are nearest-ancestor-first
/// (reverse document order).
pub fn ancestor<'a, N: Node<'a>>(n: N) -> impl Iterator<Item = N> + 'a {
    ancestor_vec(n).into_iter()
}

/// `ancestor-or-self` axis (reverse): "contains the context node and the
/// ancestors of the context node". Results are context-node-first, then
/// nearest ancestor first (reverse document order).
pub fn ancestor_or_self<'a, N: Node<'a>>(n: N) -> impl Iterator<Item = N> + 'a {
    let mut out = vec![n];
    out.extend(ancestor_vec(n));
    out.into_iter()
}

/// The parent's children of `n`, or empty if `n` is an attribute/namespace
/// node or has no parent — the shared guard for `following-sibling`/
/// `preceding-sibling`.
fn siblings_of<'a, N: Node<'a>>(n: N) -> Vec<N> {
    if is_attribute_or_namespace(n.kind()) {
        return Vec::new();
    }
    match n.parent() {
        Some(parent) => parent.children().collect(),
        None => Vec::new(),
    }
}

/// `following-sibling` axis: "contains all the following siblings of the
/// context node; if the context node is an attribute node or namespace
/// node, the following-sibling axis is empty".
pub fn following_sibling<'a, N: Node<'a>>(n: N) -> impl Iterator<Item = N> + 'a {
    let mut out = Vec::new();
    let mut found = false;
    for c in siblings_of(n) {
        if found {
            out.push(c);
        } else if c == n {
            found = true;
        }
    }
    out.into_iter()
}

/// `preceding-sibling` axis (reverse): "contains all the preceding
/// siblings of the context node; if the context node is an attribute node
/// or namespace node, the preceding-sibling axis is empty". Results are
/// nearest-preceding-sibling-first (reverse document order).
pub fn preceding_sibling<'a, N: Node<'a>>(n: N) -> impl Iterator<Item = N> + 'a {
    let mut out = Vec::new();
    for c in siblings_of(n) {
        if c == n {
            break;
        }
        out.push(c);
    }
    out.reverse();
    out.into_iter()
}

/// The full document-order listing of `n`'s document, plus `n`'s index in
/// it — the shared setup for `following`/`preceding`.
fn document_order_listing<'a, N: Node<'a>>(n: N) -> (Vec<N>, usize) {
    let all = descendant_or_self_vec(root_of(n));
    let idx = all
        .iter()
        .position(|x| *x == n)
        .expect("n must be reachable from its own document root via children()");
    (all, idx)
}

/// All nodes after `n`'s entire subtree, in document order. Never contains
/// attribute/namespace nodes, since the underlying preorder listing is
/// built purely from `children()`.
fn following_vec<'a, N: Node<'a>>(n: N) -> Vec<N> {
    let (all, idx) = document_order_listing(n);
    let subtree_len = 1 + descendant_vec(n).len();
    all.into_iter().skip(idx + subtree_len).collect()
}

/// All nodes before `n` in document order, excluding `n`'s ancestors, in
/// reverse document order (nearest-first).
fn preceding_vec<'a, N: Node<'a>>(n: N) -> Vec<N> {
    let (all, idx) = document_order_listing(n);
    let ancestors = ancestor_vec(n);
    let mut out: Vec<N> = all[..idx]
        .iter()
        .copied()
        .filter(|c| !ancestors.contains(c))
        .collect();
    out.reverse();
    out
}

/// The owner element of an attribute/namespace node — used by `following`/
/// `preceding` to reduce a context node that is itself an attribute or
/// namespace node to its owner element.
fn owner_element<'a, N: Node<'a>>(n: N) -> N {
    n.parent()
        .expect("attribute/namespace node must have an owner element as parent")
}

/// `following` axis: "contains all nodes in the same document as the
/// context node that are after the context node in document order,
/// excluding any descendants and excluding attribute nodes and namespace
/// nodes".
///
/// Attribute/namespace nodes are never children, so they never appear in
/// the underlying preorder listing at all — but per document order they
/// still sit *before* their owner element's children, so a context node
/// that is itself an attribute/namespace node is handled by reducing to
/// its owner element: `following(attr) == descendant(owner) ++
/// following(owner)`.
pub fn following<'a, N: Node<'a>>(n: N) -> impl Iterator<Item = N> + 'a {
    let out: Vec<N> = if is_attribute_or_namespace(n.kind()) {
        let owner = owner_element(n);
        let mut v = descendant_vec(owner);
        v.extend(following_vec(owner));
        v
    } else {
        following_vec(n)
    };
    out.into_iter()
}

/// `preceding` axis (reverse): "contains all nodes in the same document as
/// the context node that are before the context node in document order,
/// excluding any ancestors and excluding attribute nodes and namespace
/// nodes".
///
/// A context node that is itself an attribute/namespace node reduces to
/// its owner element: since attribute/namespace nodes are excluded from
/// the result set regardless, and everything else before the attribute in
/// document order is exactly everything before its owner element,
/// `preceding(attr) == preceding(owner)`.
pub fn preceding<'a, N: Node<'a>>(n: N) -> impl Iterator<Item = N> + 'a {
    let out: Vec<N> = if is_attribute_or_namespace(n.kind()) {
        let owner = owner_element(n);
        preceding_vec(owner)
    } else {
        preceding_vec(n)
    };
    out.into_iter()
}

/// Runs `collect` if `n` is an element, otherwise yields nothing — the
/// shared "axis is empty unless the context node is an element" guard for
/// `attribute`/`namespace`.
fn collect_if_element<'a, N: Node<'a>>(n: N, collect: impl FnOnce(N) -> Vec<N>) -> Vec<N> {
    if n.kind() == NodeKind::Element {
        collect(n)
    } else {
        Vec::new()
    }
}

/// `attribute` axis: "contains the attributes of the context node; the
/// axis will be empty unless the context node is an element".
pub fn attribute<'a, N: Node<'a>>(n: N) -> impl Iterator<Item = N> + 'a {
    collect_if_element(n, |n| n.attributes().collect()).into_iter()
}

/// `namespace` axis: "contains the namespace nodes of the context node;
/// the axis will be empty unless the context node is an element".
pub fn namespace<'a, N: Node<'a>>(n: N) -> impl Iterator<Item = N> + 'a {
    collect_if_element(n, |n| n.namespaces().collect()).into_iter()
}

/// `self` axis: "contains just the context node itself".
pub fn self_axis<'a, N: Node<'a>>(n: N) -> impl Iterator<Item = N> + 'a {
    std::iter::once(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::fixture::build;

    #[test]
    fn child_axis_on_nested_tree() {
        let f = build();
        let body = f.doc.node(f.body);
        let got: Vec<_> = child(body).collect();
        assert_eq!(
            got,
            vec![
                f.doc.node(f.text0),
                f.doc.node(f.span),
                f.doc.node(f.comment),
                f.doc.node(f.pi),
                f.doc.node(f.text2),
            ]
        );
    }

    #[test]
    fn descendant_axis_on_nested_tree() {
        let f = build();
        let html = f.doc.node(f.html);
        let got: Vec<_> = descendant(html).collect();
        assert_eq!(
            got,
            vec![
                f.doc.node(f.body),
                f.doc.node(f.text0),
                f.doc.node(f.span),
                f.doc.node(f.text1),
                f.doc.node(f.comment),
                f.doc.node(f.pi),
                f.doc.node(f.text2),
            ]
        );
    }

    #[test]
    fn descendant_or_self_axis_on_nested_tree() {
        let f = build();
        let span = f.doc.node(f.span);
        let got: Vec<_> = descendant_or_self(span).collect();
        assert_eq!(got, vec![span, f.doc.node(f.text1)]);
    }

    #[test]
    fn parent_ancestor_ancestor_or_self_from_leaf_nearest_first() {
        let f = build();
        let text1 = f.doc.node(f.text1); // leaf: text inside span inside body inside html
        let span = f.doc.node(f.span);
        let body = f.doc.node(f.body);
        let html = f.doc.node(f.html);
        let root = f.doc.node(0);

        assert_eq!(parent(text1).collect::<Vec<_>>(), vec![span]);
        assert_eq!(
            ancestor(text1).collect::<Vec<_>>(),
            vec![span, body, html, root],
            "ancestor must be nearest-parent-first, not root-first"
        );
        assert_eq!(
            ancestor_or_self(text1).collect::<Vec<_>>(),
            vec![text1, span, body, html, root]
        );
    }

    #[test]
    fn ancestor_of_root_is_empty() {
        let f = build();
        let root = f.doc.node(0);
        assert_eq!(ancestor(root).collect::<Vec<_>>(), Vec::new());
    }

    #[test]
    fn following_and_preceding_sibling_order_on_multiple_siblings() {
        let f = build();
        let comment = f.doc.node(f.comment);
        // body's children: text0, span, comment, pi, text2
        assert_eq!(
            following_sibling(comment).collect::<Vec<_>>(),
            vec![f.doc.node(f.pi), f.doc.node(f.text2)]
        );
        assert_eq!(
            preceding_sibling(comment).collect::<Vec<_>>(),
            vec![f.doc.node(f.span), f.doc.node(f.text0)],
            "preceding-sibling must be nearest-sibling-first"
        );
    }

    #[test]
    fn following_excludes_attribute_and_namespace_nodes() {
        let f = build();
        let span = f.doc.node(f.span);
        // span's own `data-x` attribute sits in document order strictly
        // between span and text1, i.e. it is after span but is not among
        // span's descendants (the descendant axis never contains attribute
        // nodes) — a `following` that excludes only descendants without
        // also checking node kind would wrongly include it here.
        let got: Vec<_> = following(span).collect();
        assert_eq!(
            got,
            vec![f.doc.node(f.comment), f.doc.node(f.pi), f.doc.node(f.text2)]
        );
        assert!(!got.contains(&f.doc.node(f.span_attr)));
    }

    #[test]
    fn preceding_excludes_attribute_and_namespace_nodes() {
        let f = build();
        let text0 = f.doc.node(f.text0);
        // A naive ancestor-exclusion-only `preceding` would wrongly include
        // html's and body's attribute/namespace nodes here, since none of
        // them are literal ancestors of text0.
        let got: Vec<_> = preceding(text0).collect();
        assert_eq!(got, Vec::new());
    }

    #[test]
    fn preceding_excludes_a_preceding_siblings_own_attribute_node() {
        let f = build();
        let text1 = f.doc.node(f.text1);
        // span's `data-x` attribute is before text1 in document order and
        // is not an ancestor of text1 (span, its owner, is the ancestor;
        // the attribute node itself is not on the ancestor chain) — a
        // `preceding` that excludes only ancestors without also checking
        // node kind would wrongly include it here.
        let got: Vec<_> = preceding(text1).collect();
        assert_eq!(got, vec![f.doc.node(f.text0)]);
        assert!(!got.contains(&f.doc.node(f.span_attr)));
    }

    #[test]
    fn preceding_reverse_document_order_excluding_ancestors() {
        let f = build();
        let text2 = f.doc.node(f.text2);
        let got: Vec<_> = preceding(text2).collect();
        assert_eq!(
            got,
            vec![
                f.doc.node(f.pi),
                f.doc.node(f.comment),
                f.doc.node(f.text1),
                f.doc.node(f.span),
                f.doc.node(f.text0),
            ],
            "preceding must be reverse document order and exclude ancestors (body, html, root)"
        );
        assert!(!got.contains(&f.doc.node(f.body)));
        assert!(!got.contains(&f.doc.node(f.html)));
    }

    #[test]
    fn attribute_and_namespace_axes_on_element_with_both() {
        let f = build();
        let html = f.doc.node(f.html);
        assert_eq!(
            attribute(html).collect::<Vec<_>>(),
            vec![f.doc.node(f.html_lang), f.doc.node(f.html_id)]
        );
        assert_eq!(
            namespace(html).collect::<Vec<_>>(),
            vec![f.doc.node(f.html_ns)]
        );

        let body = f.doc.node(f.body);
        assert_eq!(
            attribute(body).collect::<Vec<_>>(),
            vec![f.doc.node(f.body_class)]
        );
        assert_eq!(
            namespace(body).collect::<Vec<_>>(),
            vec![f.doc.node(f.body_ns)]
        );
    }

    #[test]
    fn self_axis_is_trivial() {
        let f = build();
        let body = f.doc.node(f.body);
        assert_eq!(self_axis(body).collect::<Vec<_>>(), vec![body]);
    }

    #[test]
    fn attribute_node_has_no_attribute_namespace_or_sibling_axes() {
        let f = build();
        let lang = f.doc.node(f.html_lang);
        assert_eq!(attribute(lang).collect::<Vec<_>>(), Vec::new());
        assert_eq!(namespace(lang).collect::<Vec<_>>(), Vec::new());
        assert_eq!(following_sibling(lang).collect::<Vec<_>>(), Vec::new());
        assert_eq!(preceding_sibling(lang).collect::<Vec<_>>(), Vec::new());
    }

    #[test]
    fn following_and_preceding_from_an_attribute_context_node() {
        let f = build();
        let lang = f.doc.node(f.html_lang);
        // following(attr) reduces to descendant(owner) ++ following(owner);
        // for html's `lang` attribute that's everything inside html.
        assert_eq!(
            following(lang).collect::<Vec<_>>(),
            vec![
                f.doc.node(f.body),
                f.doc.node(f.text0),
                f.doc.node(f.span),
                f.doc.node(f.text1),
                f.doc.node(f.comment),
                f.doc.node(f.pi),
                f.doc.node(f.text2),
            ]
        );
        // preceding(attr) reduces to preceding(owner); html has nothing
        // before it.
        assert_eq!(preceding(lang).collect::<Vec<_>>(), Vec::new());
    }
}
