# xpath-eval

A pure-Rust implementation of [XPath 1.0](https://www.w3.org/TR/1999/REC-xpath-19991116/)
— parses an XPath expression and evaluates it against a document.

Generic and standalone — not tied to HTML, XML parsing, Schematron, or any
specific document type. Callers provide their own parsed document via a
trait; this crate only implements the XPath side (expression parsing,
the data model view over the caller's tree, and evaluation).

## Why this exists

This crate stands on its own: XPath 1.0 evaluation is a distinct,
independently useful capability, not inherently coupled to Schematron, HTML,
or any other single consumer. It was split out as its own crate rather than
folded into another project specifically so it stays usable outside of
whatever prompted its creation.

That said, its origin is concrete: [`schematron-engine`](https://github.com/casoon/schematron-engine)
needs it to evaluate `test="..."` XPath expressions from Schematron rules,
and [`html-conform`](https://github.com/casoon/html-conform) is the first
real-world application that needs `xpath-eval` and `schematron-engine`
*together* — as a replacement for `xmloxide`'s bundled XPath/Schematron
support in that project's assertion layer (Schicht 3). See `html-conform`'s
`plan/DECISIONS.md` for the full context. That combination is one use case
among potentially others, not this crate's reason for existing.

## Status

Implemented: the lexer/parser, the full location-path/predicate evaluation
core (§2), and 25 of the 26 XPath 1.0 Core Function Library functions
(§4) — every one except `id()`. 142 tests pass, `clippy` and `fmt` are
clean.

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

- **`id()` is not implemented.** The XPath 1.0 data model requires knowing
  which attributes are DTD-declared `ID`-typed, which the `Node`/`Document`
  traits have no way to express; calling `id()` returns
  `EvalError::UnknownFunction`. Everything else in §4 is implemented.
- **Namespace-prefix resolution is opt-in.** Without a `namespaces` hook on
  `EvaluationContext`, a prefixed name test (`p:foo`) falls back to
  comparing the expression's raw prefix string directly against a node's
  namespace URI — only correct by coincidence. Supply a real resolver hook
  for spec-correct prefix→URI resolution.
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
xpath-eval = "0.1"
```

## License

MIT — see [LICENSE](LICENSE).
