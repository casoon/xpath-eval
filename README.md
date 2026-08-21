# xpath-eval

A pure-Rust implementation of [XPath 1.0](https://www.w3.org/TR/1999/REC-xpath-19991116/)
— parses an XPath expression and evaluates it against a document.

Generic and standalone — not tied to HTML, XML parsing, Schematron, or any
specific document type. Callers provide their own parsed document via a
trait; this crate only implements the XPath side (expression parsing,
the data model view over the caller's tree, and evaluation).

## Status

Implemented: the lexer/parser, the full location-path/predicate evaluation
core (§2), and the complete XPath 1.0 Core Function Library (§4, all 27
functions, including `id()`). 147 tests pass, `clippy` and `fmt` are clean.

## Example

```rust
use xpath_eval::{Document, EvaluationContext, evaluate, parse};

// A caller brings their own tree, implementing `Node`/`Document` over it —
// this crate never parses or builds one itself. See `Node`'s doc comments
// for what each method must return.
# fn example<D: Document>(doc: &D) -> Result<(), Box<dyn std::error::Error>> {
let expr = parse("//item[@id='42']/name/text()")?;
let ctx = EvaluationContext::new(doc.root());
let result = evaluate(&expr, &ctx)?;
println!("{}", result.to_xpath_string());
# Ok(())
# }
```

`EvaluationContext::new` covers the common case (no variables, no namespace
context). For XPath variables (`$x`) or prefixed name tests (`p:foo`) that
need real namespace resolution, build an `EvaluationContext` directly and
supply a `variables`/`namespaces` lookup hook — see their doc comments.

## Scope and known limitations

- **`id()` needs `Node::is_id_attribute()` to be overridden to do anything
  useful.** The XPath 1.0 data model derives ID-ness from a DTD or schema;
  most `Node` implementations have no such information, so the trait's
  default (`false` everywhere) means `id()` is a real function that simply
  never matches. Override `is_id_attribute()` on your `Attribute`-kind
  nodes if you know which attributes are IDs.
- **Namespace-prefix resolution requires a `namespaces` hook.** Without one
  on `EvaluationContext`, a prefixed name test (`p:foo`) never matches any
  node — it does not fall back to guessing. Supply a resolver hook for
  spec-correct prefix→URI resolution.
- **`following`/`preceding` are full document-order axes** (§2.3): each
  walk touches the potentially-large before/after portion of the whole
  tree, not just nearby nodes. Fine for typical documents; be aware of the
  cost on very large trees.
- Implementing `Node` correctly requires `document_order` to be a genuine
  total order consistent with `children()`/`attributes()`/`namespaces()` —
  see the trait's doc comments for the exact ordering rules (element before
  its namespace nodes before its attribute nodes before its children).

## Installation

```toml
[dependencies]
xpath-eval = "0.2"
```

## License

MIT — see [LICENSE](LICENSE).
