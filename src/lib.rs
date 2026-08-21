#![doc = include_str!("../README.md")]

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
