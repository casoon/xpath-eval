//! The XPath 1.0 data model (§5): node kinds, expanded names, and the
//! `Node`/`Document` traits a caller implements over their own tree.
//!
//! This module only defines the data model — the 13 axes derived on top of
//! it live in `axes.rs`, not here (see `plan/03-document-trait.md`).

use std::cmp::Ordering;

/// An XPath 1.0 node type (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Root,
    Element,
    Attribute,
    Namespace,
    ProcessingInstruction,
    Comment,
    Text,
}

/// An expanded name (namespace URI + local name), §5.2/§5.3/§5.4/§5.5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedName {
    pub namespace_uri: Option<String>,
    pub local_name: String,
}

/// A node in the caller's tree, viewed as an XPath 1.0 data-model node.
///
/// `Copy` because axis iterators (`axes.rs`) produce many short-lived
/// handles — modeled after `roxmltree::Node<'a>`, not a `NodeId`/arena
/// scheme.
///
/// Methods take `self` by value (not `&self`) despite the `Copy` bound —
/// deliberately, so the returned `impl Iterator<Item = Self> + 'a` only
/// captures `'a` and not an additional borrow of a locally-owned handle
/// (Rust's return-position-impl-trait-in-traits rules otherwise implicitly
/// capture the `&self` lifetime too, which conflicts with callers that
/// build axis iterators from an owned, short-lived `N`).
pub trait Node<'a>: Copy + Eq + 'a {
    /// The node's type (§5).
    fn kind(self) -> NodeKind;

    /// The node's parent, if any. Root nodes have no parent. Attribute and
    /// namespace nodes have a parent (their owner element) despite not
    /// being among that element's `children()`.
    fn parent(self) -> Option<Self>;

    /// The node's children. Non-empty only for `Root`/`Element` (§5.1/§5.2)
    /// — attribute and namespace nodes are never children of anything.
    fn children(self) -> impl Iterator<Item = Self> + 'a;

    /// The element's attribute nodes. Non-empty only for `Element` (§5.3).
    /// Not part of `children()` — see the document-order rule below.
    fn attributes(self) -> impl Iterator<Item = Self> + 'a;

    /// The element's namespace nodes. Non-empty only for `Element` (§5.4).
    /// Not part of `children()`.
    fn namespaces(self) -> impl Iterator<Item = Self> + 'a;

    /// The node's expanded name. `None` for `Root`/`Comment`/`Text`, which
    /// have no name; `Some` for `Element`/`Attribute`/`Namespace`/
    /// `ProcessingInstruction` per the §5 table.
    fn expanded_name(self) -> Option<ExpandedName>;

    /// The node's string-value, computed per node kind per the §5 table
    /// (e.g. Element/Root: concatenation of the string-values of all
    /// descendant text nodes, in document order).
    fn string_value(self) -> String;

    /// Total order per document order (§5): an element occurs before its
    /// namespace nodes, which occur before its attribute nodes, which
    /// occur before its children, each subtree recursively in document
    /// order. (Relative order among an element's own namespace nodes, and
    /// among its own attribute nodes, is implementation-defined — it only
    /// needs to be *some* stable order.) Must be consistent with
    /// `children()`/`attributes()`/`namespaces()`.
    fn document_order(self, other: Self) -> Ordering;

    /// Whether this attribute node is of type `ID` (§5.3, §4.1's `id()`) —
    /// i.e. whether XPath's `id()` function should be able to find this
    /// attribute's owner element by this attribute's string-value. The
    /// XPath 1.0 data model derives this from a DTD or schema; most callers
    /// have no such information, so the default is `false` everywhere
    /// (`id()` then always returns an empty node-set, never an error — the
    /// function itself is always implemented). Callers that do have ID
    /// information (DTD validation, `xml:id`, a known convention) should
    /// override this on their `Attribute`-kind nodes. Meaningless (and
    /// never called by this crate) on non-`Attribute` nodes.
    fn is_id_attribute(self) -> bool {
        false
    }
}

/// An XPath 1.0 document: gives access to the root node.
pub trait Document {
    type N<'a>: Node<'a>
    where
        Self: 'a;

    fn root(&self) -> Self::N<'_>;
}

/// A minimal in-memory reference tree, used only to prove `Node`/`Document`
/// are implementable and to drive the axis tests in `axes.rs`. Not a
/// production tree type, not part of the public API.
#[cfg(test)]
pub(crate) mod fixture {
    use super::{Document, ExpandedName, Node, NodeKind};
    use std::cmp::Ordering;

    #[derive(Debug)]
    struct NodeData {
        kind: NodeKind,
        name: Option<ExpandedName>,
        text: Option<String>,
        parent: Option<usize>,
        children: Vec<usize>,
        attributes: Vec<usize>,
        namespaces: Vec<usize>,
    }

    impl NodeData {
        fn new(kind: NodeKind, parent: Option<usize>) -> Self {
            NodeData {
                kind,
                name: None,
                text: None,
                parent,
                children: Vec::new(),
                attributes: Vec::new(),
                namespaces: Vec::new(),
            }
        }
    }

    #[derive(Debug)]
    struct Arena {
        nodes: Vec<NodeData>,
    }

    /// Builds a fixture tree. Calls must add an element's namespaces, then
    /// its attributes, then its children (each fully built before moving
    /// to the next sibling) so that arena indices come out in true
    /// document order — `document_order()` is then just an index compare.
    struct Builder {
        nodes: Vec<NodeData>,
    }

    impl Builder {
        fn push(&mut self, data: NodeData) -> usize {
            let idx = self.nodes.len();
            self.nodes.push(data);
            idx
        }

        fn new_root(&mut self) -> usize {
            self.push(NodeData::new(NodeKind::Root, None))
        }

        fn add_element(&mut self, parent: usize, local: &str) -> usize {
            let mut data = NodeData::new(NodeKind::Element, Some(parent));
            data.name = Some(ExpandedName {
                namespace_uri: None,
                local_name: local.to_string(),
            });
            let idx = self.push(data);
            self.nodes[parent].children.push(idx);
            idx
        }

        fn add_namespace(&mut self, owner: usize, prefix: &str, uri: &str) -> usize {
            let mut data = NodeData::new(NodeKind::Namespace, Some(owner));
            data.name = Some(ExpandedName {
                namespace_uri: None,
                local_name: prefix.to_string(),
            });
            data.text = Some(uri.to_string());
            let idx = self.push(data);
            self.nodes[owner].namespaces.push(idx);
            idx
        }

        fn add_attribute(&mut self, owner: usize, local: &str, value: &str) -> usize {
            let mut data = NodeData::new(NodeKind::Attribute, Some(owner));
            data.name = Some(ExpandedName {
                namespace_uri: None,
                local_name: local.to_string(),
            });
            data.text = Some(value.to_string());
            let idx = self.push(data);
            self.nodes[owner].attributes.push(idx);
            idx
        }

        fn add_text(&mut self, parent: usize, text: &str) -> usize {
            let mut data = NodeData::new(NodeKind::Text, Some(parent));
            data.text = Some(text.to_string());
            let idx = self.push(data);
            self.nodes[parent].children.push(idx);
            idx
        }

        fn add_comment(&mut self, parent: usize, text: &str) -> usize {
            let mut data = NodeData::new(NodeKind::Comment, Some(parent));
            data.text = Some(text.to_string());
            let idx = self.push(data);
            self.nodes[parent].children.push(idx);
            idx
        }

        fn add_pi(&mut self, parent: usize, target: &str, content: &str) -> usize {
            let mut data = NodeData::new(NodeKind::ProcessingInstruction, Some(parent));
            data.name = Some(ExpandedName {
                namespace_uri: None,
                local_name: target.to_string(),
            });
            data.text = Some(content.to_string());
            let idx = self.push(data);
            self.nodes[parent].children.push(idx);
            idx
        }
    }

    pub(crate) struct TestDoc {
        arena: Arena,
    }

    impl TestDoc {
        pub(crate) fn node(&self, idx: usize) -> TestNode<'_> {
            TestNode {
                arena: &self.arena,
                idx,
            }
        }
    }

    impl Document for TestDoc {
        type N<'a> = TestNode<'a>;

        fn root(&self) -> Self::N<'_> {
            self.node(0)
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub(crate) struct TestNode<'a> {
        arena: &'a Arena,
        idx: usize,
    }

    impl<'a> PartialEq for TestNode<'a> {
        fn eq(&self, other: &Self) -> bool {
            std::ptr::eq(self.arena, other.arena) && self.idx == other.idx
        }
    }

    impl<'a> Eq for TestNode<'a> {}

    impl<'a> Node<'a> for TestNode<'a> {
        fn kind(self) -> NodeKind {
            self.arena.nodes[self.idx].kind
        }

        fn parent(self) -> Option<Self> {
            self.arena.nodes[self.idx].parent.map(|p| TestNode {
                arena: self.arena,
                idx: p,
            })
        }

        fn children(self) -> impl Iterator<Item = Self> + 'a {
            let arena = self.arena;
            self.arena.nodes[self.idx]
                .children
                .clone()
                .into_iter()
                .map(move |i| TestNode { arena, idx: i })
        }

        fn attributes(self) -> impl Iterator<Item = Self> + 'a {
            let arena = self.arena;
            self.arena.nodes[self.idx]
                .attributes
                .clone()
                .into_iter()
                .map(move |i| TestNode { arena, idx: i })
        }

        fn namespaces(self) -> impl Iterator<Item = Self> + 'a {
            let arena = self.arena;
            self.arena.nodes[self.idx]
                .namespaces
                .clone()
                .into_iter()
                .map(move |i| TestNode { arena, idx: i })
        }

        fn expanded_name(self) -> Option<ExpandedName> {
            self.arena.nodes[self.idx].name.clone()
        }

        fn string_value(self) -> String {
            let data = &self.arena.nodes[self.idx];
            match data.kind {
                NodeKind::Root | NodeKind::Element => {
                    let mut out = String::new();
                    collect_text(self.arena, self.idx, &mut out);
                    out
                }
                NodeKind::Attribute
                | NodeKind::Namespace
                | NodeKind::ProcessingInstruction
                | NodeKind::Comment
                | NodeKind::Text => data.text.clone().unwrap_or_default(),
            }
        }

        fn document_order(self, other: Self) -> Ordering {
            self.idx.cmp(&other.idx)
        }
    }

    fn collect_text(arena: &Arena, idx: usize, out: &mut String) {
        for &child in &arena.nodes[idx].children {
            match arena.nodes[child].kind {
                NodeKind::Text => out.push_str(arena.nodes[child].text.as_deref().unwrap_or("")),
                NodeKind::Element => collect_text(arena, child, out),
                _ => {}
            }
        }
    }

    /// Named handles into the fixture tree built by [`build`]:
    ///
    /// ```text
    /// Root
    ///  └─ html (ns: xml=...; attrs: lang="en", id="root-el")
    ///      └─ body (ns: x=...; attrs: class="main")
    ///          ├─ text0 "Hello "
    ///          ├─ span (attrs: data-x="1")
    ///          │   └─ text1 "World"
    ///          ├─ comment "note"
    ///          ├─ pi proc "data"
    ///          └─ text2 "!"
    /// ```
    ///
    /// `span`'s `data-x` attribute exists specifically as a trap for
    /// `following`/`preceding`: it sits in document order strictly between
    /// `span` and `text1`, so it is neither an ancestor nor a descendant
    /// of `text0`/`text2` — an implementation that excludes attribute/
    /// namespace nodes from those axes only by filtering out descendants/
    /// ancestors (instead of also filtering by node kind) would wrongly
    /// let it leak into `following(text0)` or `preceding(text2)`.
    pub(crate) struct Fixture {
        pub doc: TestDoc,
        pub html: usize,
        pub html_ns: usize,
        pub html_lang: usize,
        pub html_id: usize,
        pub body: usize,
        pub body_ns: usize,
        pub body_class: usize,
        pub text0: usize,
        pub span: usize,
        pub span_attr: usize,
        pub text1: usize,
        pub comment: usize,
        pub pi: usize,
        pub text2: usize,
    }

    pub(crate) fn build() -> Fixture {
        let mut b = Builder { nodes: Vec::new() };
        let root = b.new_root();
        let html = b.add_element(root, "html");
        let html_ns = b.add_namespace(html, "xml", "http://www.w3.org/XML/1998/namespace");
        let html_lang = b.add_attribute(html, "lang", "en");
        let html_id = b.add_attribute(html, "id", "root-el");
        let body = b.add_element(html, "body");
        let body_ns = b.add_namespace(body, "x", "urn:x");
        let body_class = b.add_attribute(body, "class", "main");
        let text0 = b.add_text(body, "Hello ");
        let span = b.add_element(body, "span");
        let span_attr = b.add_attribute(span, "data-x", "1");
        let text1 = b.add_text(span, "World");
        let comment = b.add_comment(body, "note");
        let pi = b.add_pi(body, "proc", "data");
        let text2 = b.add_text(body, "!");
        Fixture {
            doc: TestDoc {
                arena: Arena { nodes: b.nodes },
            },
            html,
            html_ns,
            html_lang,
            html_id,
            body,
            body_ns,
            body_class,
            text0,
            span,
            span_attr,
            text1,
            comment,
            pi,
            text2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::build;
    use super::{Node, NodeKind};

    #[test]
    fn string_value_text() {
        let f = build();
        assert_eq!(f.doc.node(f.text0).string_value(), "Hello ");
    }

    #[test]
    fn string_value_comment() {
        let f = build();
        assert_eq!(f.doc.node(f.comment).string_value(), "note");
    }

    #[test]
    fn string_value_processing_instruction() {
        let f = build();
        assert_eq!(f.doc.node(f.pi).string_value(), "data");
    }

    #[test]
    fn string_value_attribute() {
        let f = build();
        assert_eq!(f.doc.node(f.html_lang).string_value(), "en");
    }

    #[test]
    fn string_value_namespace() {
        let f = build();
        assert_eq!(
            f.doc.node(f.html_ns).string_value(),
            "http://www.w3.org/XML/1998/namespace"
        );
    }

    #[test]
    fn string_value_element_concatenates_nested_descendant_text_in_document_order() {
        let f = build();
        // span is nested one level deeper than text0/text2 — string-value
        // must still concatenate across depths, in document order.
        assert_eq!(f.doc.node(f.span).string_value(), "World");
        assert_eq!(f.doc.node(f.body).string_value(), "Hello World!");
        assert_eq!(f.doc.node(f.html).string_value(), "Hello World!");
    }

    #[test]
    fn string_value_root_concatenates_all_descendant_text() {
        let f = build();
        assert_eq!(f.doc.node(0).string_value(), "Hello World!");
    }

    #[test]
    fn expanded_name_per_kind() {
        let f = build();
        assert!(f.doc.node(0).expanded_name().is_none()); // Root
        assert_eq!(
            f.doc.node(f.html).expanded_name().unwrap().local_name,
            "html"
        );
        assert_eq!(
            f.doc.node(f.html_lang).expanded_name().unwrap().local_name,
            "lang"
        );
        assert_eq!(
            f.doc.node(f.html_ns).expanded_name().unwrap().local_name,
            "xml"
        );
        assert_eq!(f.doc.node(f.pi).expanded_name().unwrap().local_name, "proc");
        assert!(f.doc.node(f.comment).expanded_name().is_none());
        assert!(f.doc.node(f.text0).expanded_name().is_none());
    }

    #[test]
    fn kind_per_node() {
        let f = build();
        assert_eq!(f.doc.node(0).kind(), NodeKind::Root);
        assert_eq!(f.doc.node(f.html).kind(), NodeKind::Element);
        assert_eq!(f.doc.node(f.html_lang).kind(), NodeKind::Attribute);
        assert_eq!(f.doc.node(f.html_ns).kind(), NodeKind::Namespace);
        assert_eq!(f.doc.node(f.pi).kind(), NodeKind::ProcessingInstruction);
        assert_eq!(f.doc.node(f.comment).kind(), NodeKind::Comment);
        assert_eq!(f.doc.node(f.text0).kind(), NodeKind::Text);
    }

    #[test]
    fn document_order_matches_tree_structure() {
        let f = build();
        use std::cmp::Ordering;
        let root = f.doc.node(0);
        let html = f.doc.node(f.html);
        let body = f.doc.node(f.body);
        let text0 = f.doc.node(f.text0);
        assert_eq!(root.document_order(html), Ordering::Less);
        assert_eq!(html.document_order(body), Ordering::Less);
        assert_eq!(body.document_order(text0), Ordering::Less);
        assert_eq!(html.document_order(html), Ordering::Equal);
    }
}
