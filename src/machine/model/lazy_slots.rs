//! Lazy-slot kinds: which part kinds a builtin form's child slot captures raw instead of evaluating.
//!
//! A bare `(…)` evaluates before its parent dispatches — everywhere except a lazy slot of a fixed
//! builtin form. Which slots those are is a **seal-time** fact, not a dispatch-time one: the
//! [`lazy_slots`](crate::machine::model::key_spec::Form::lazy_slots) of the node's
//! [`FORMS`](crate::machine::model::key_spec::FORMS) entry are the single source of truth, a node's
//! construction resolves that entry, and the scheduler reads it to decide child submission. So
//! dispatch selects among overloads over values that have already landed, and a reader can tell
//! locally whether a group runs.
//!
//! Lazy-slot declaration is available only to builtin registration — a user `FN` signature never
//! receives a raw unquoted group, and a `:KExpression` parameter of one is an ordinary eager value
//! parameter satisfied by a `#(…)` literal or any `KExpression`-valued expression. The entries are
//! pinned to the live builtin signatures by the table⟺registration property: index `i` of bucket
//! `k` carries kind `K` iff some builtin overload registered under `k` types slot `i` with `K`'s
//! slot type, or with a union carrying it as a member — one union-typed slot admits every carrier
//! spelling it lists, and each contributes its own kind here.

/// The part kinds a lazy slot can capture raw, as a set. One kind per raw-capture slot type:
/// `CODE` for a `:KExpression` slot (an `(…)` group or a `#(…)` quote — one spelling in a lazy
/// slot, both captured raw), `TYPE_EXPR` for `:SigiledTypeExpr` (`:(…)`), `RECORD_TYPE` for
/// `:RecordType` (`:{…}`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LazyKinds(u8);

impl LazyKinds {
    pub const EMPTY: LazyKinds = LazyKinds(0);
    pub const CODE: LazyKinds = LazyKinds(1);
    pub const TYPE_EXPR: LazyKinds = LazyKinds(1 << 1);
    pub const RECORD_TYPE: LazyKinds = LazyKinds(1 << 2);

    pub const fn with(self, other: LazyKinds) -> LazyKinds {
        LazyKinds(self.0 | other.0)
    }

    pub const fn contains(self, other: LazyKinds) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

#[cfg(test)]
mod tests;
