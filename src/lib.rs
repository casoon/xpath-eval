//! A pure-Rust XPath 1.0 expression parser and evaluator, generic over the
//! document model.
//!
//! Phase 02: lexer + recursive-descent parser producing a structured AST.
//! Phase 03: the generic document/node data model and the 13 XPath axes.
//! Phase 04: the value model, type conversions, operators, and full
//! location-path/predicate evaluation — see `plan/` for the roadmap.

mod ast;
mod axes;
mod document;
mod eval;
mod functions;
mod lexer;
mod parser;
mod value;

pub use ast::*;
pub use axes::{
    ancestor, ancestor_or_self, attribute, child, descendant, descendant_or_self, following,
    following_sibling, namespace, parent, preceding, preceding_sibling, self_axis,
};
pub use document::{Document, ExpandedName, Node, NodeKind};
pub use eval::{EvalError, EvaluationContext, evaluate};
pub use parser::{ParseError, parse};
pub use value::Value;
