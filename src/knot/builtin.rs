//! A builtin overload as a knot node: a function value whose body is native.
//!
//! The record — the overload's registered shape and its index in the embedder's native table — is
//! laid down once in program storage beside the builtin table that holds it, and the node points at
//! it. A builtin equals only itself: its identity is the record's address, and it holds no
//! captures. A copy carries the node as-is, since the record outlives every cell. The knot layer
//! never runs one: `id` names a native only the embedder knows.

use crate::memory::{KnotPlan, Writer, resident};
use crate::type_lattice::KType;
use crate::values::Weight;

use super::{Knotted, Node};

/// A builtin overload: what a builtin node points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinFunction {
    ktype: KType,
    id: u32,
}

impl BuiltinFunction {
    /// The expression shape the overload is registered at, which is the type it carries.
    pub fn ktype(&self) -> KType {
        self.ktype
    }

    /// The overload's index in the embedder's native table.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// What rebuilding a builtin's one-node knot writes: its header and the node, never the record.
    pub(super) fn knot_weight() -> Weight {
        Weight::flat::<usize>().plus(Weight::flat::<Node<'static, 'static>>())
    }
}

/// The builtin door: the overload `id` of the embedder's native table, registered at the expression
/// shape `ktype`, laid down with its node as a one-node knot in program storage, which `writer`
/// fills.
pub fn builtin<'graph>(writer: Writer<'graph>, ktype: KType, id: u32) -> Knotted<'graph, 'graph> {
    let record = resident(writer, BuiltinFunction { ktype, id });
    let knot = KnotPlan::new(1).tie(writer, |_| Node::Builtin(record));
    Knotted::of(knot, 0)
}
