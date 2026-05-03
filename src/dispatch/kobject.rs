use std::collections::HashMap;
use std::rc::Rc;

use crate::parse::kexpression::KExpression;
use super::arena::CallArena;
use super::ktraits::{Parseable, Serializable};
use super::kfunction::{KFunction, KType};
use super::scope::KFuture;

/// Runtime value: scalars, collections, an unevaluated expression, a bound-but-unrun task, or a
/// reference to a function in some scope. The universal value type that `KFunction`s consume
/// and produce; implements `Parseable` so values can be compared and rendered uniformly.
///
/// `List(Rc<Vec<...>>)` / `Dict(Rc<HashMap<...>>)`: composite payloads are reference-counted
/// so deep_clone (and the scheduler's lift-on-return walk) can `Rc::clone` instead of
/// rebuilding. The contract is "Koan's collections are immutable values"; a future mutable-list
/// builtin would need `Rc::make_mut` at the mutation site to clone-on-write. Subsequent lifts
/// of the same value through a return chain are O(1) for these variants once any embedded
/// `KFunction` has had its `Rc<CallArena>` attached.
///
/// `KFunction(&fn, Option<Rc<CallArena>>)`: the second field keeps the function's underlying
/// per-call arena alive when the function escaped from a per-call body (via lift-on-return).
/// `None` for the common case (builtins and top-level FNs, which live in run-root and don't
/// need a refcount). `Some(rc)` is set by the scheduler's lift logic when it detects the
/// function lives in a dying frame's arena. The Rc keeps the arena alive past the slot's
/// frame drop, so the `&KFunction` reference stays valid.
///
/// `KFuture(KFuture, Option<Rc<CallArena>>)`: same lifecycle anchor as `KFunction`. The
/// inner `KFuture` carries a bare `&KFunction` plus a bundle and parsed `KExpression` whose
/// `Future` parts are also borrowed `&KObject`s — all of which can point into a per-call
/// arena. A single `Rc<CallArena>` keeps that arena alive, transitively keeping every
/// internal reference valid. Set by `lift_kobject` when a KFuture-as-value escapes.
///
/// `TaggedUnionType(Rc<HashMap<...>>)`: a first-class tagged-union schema, mapping tag
/// names to the `KType` each tag's payload must satisfy. Built by the `UNION` builtin and
/// consumed by `TAG` (and the surface-level type-token / call-by-name construction paths)
/// to validate tagged values at construction time.
///
/// `Tagged { tag, value }`: a tagged value — one variant of a tagged union, carrying its
/// tag name and inner payload. The payload is `Rc`-shared like `List`/`Dict` to keep
/// `deep_clone` cheap and the lift-on-return walk able to skip allocation when no
/// descendant `KFunction` is in flight.
pub enum KObject<'a> {
    Number(f64),
    KString(String),
    Bool(bool),
    List(Rc<Vec<KObject<'a>>>),
    Dict(Rc<HashMap<Box<dyn Serializable + 'a>, KObject<'a>>>),
    KExpression(KExpression<'a>),
    KFuture(KFuture<'a>, Option<Rc<CallArena>>),
    KFunction(&'a KFunction<'a>, Option<Rc<CallArena>>),
    TaggedUnionType(Rc<HashMap<String, KType>>),
    Tagged {
        tag: String,
        value: Rc<KObject<'a>>,
    },
    Null,
}

impl<'a> KObject<'a> {
    /// Runtime type tag for this value. Used by the scheduler's post-call return-type check
    /// (`KType::matches_value`) and any future static-pass tooling. `KFuture` reports as
    /// `KFunction` since a bound-but-unrun call is functionally a thunk and KFutures don't
    /// escape as user-visible values today.
    pub fn ktype(&self) -> KType {
        match self {
            KObject::Number(_) => KType::Number,
            KObject::KString(_) => KType::Str,
            KObject::Bool(_) => KType::Bool,
            KObject::Null => KType::Null,
            KObject::List(_) => KType::List,
            KObject::Dict(_) => KType::Dict,
            KObject::KFunction(_, _) => KType::KFunction,
            KObject::KFuture(_, _) => KType::KFunction,
            KObject::KExpression(_) => KType::KExpression,
            KObject::TaggedUnionType(_) => KType::TaggedUnionType,
            KObject::Tagged { .. } => KType::Tagged,
        }
    }

    /// Independent-but-cheap clone. Composite payloads are `Rc`-shared (the immutability
    /// contract on the `List`/`Dict` variants makes structural sharing safe), so cloning
    /// those is `Rc::clone` rather than a recursive walk. `KFuture` and `KExpression` carry
    /// their own clone semantics; `KFunction` preserves its `Rc<CallArena>` (if any) so a
    /// kept-alive per-call arena stays alive in the clone. `Dict` keys are cloned through
    /// `Serializable::clone_box` only when the surrounding `Rc` is being rebuilt — that
    /// happens in `lift_kobject`, not here.
    pub fn deep_clone(&self) -> KObject<'a> {
        match self {
            KObject::Number(n) => KObject::Number(*n),
            KObject::KString(s) => KObject::KString(s.clone()),
            KObject::Bool(b) => KObject::Bool(*b),
            KObject::Null => KObject::Null,
            KObject::List(items) => KObject::List(Rc::clone(items)),
            KObject::Dict(entries) => KObject::Dict(Rc::clone(entries)),
            KObject::KExpression(e) => KObject::KExpression(e.clone()),
            KObject::KFuture(t, frame) => KObject::KFuture(t.deep_clone(), frame.clone()),
            KObject::KFunction(f, frame) => KObject::KFunction(*f, frame.clone()),
            KObject::TaggedUnionType(schema) => KObject::TaggedUnionType(Rc::clone(schema)),
            KObject::Tagged { tag, value } => KObject::Tagged {
                tag: tag.clone(),
                value: Rc::clone(value),
            },
        }
    }
}

impl<'a> Parseable for KObject<'a> {
    fn equal(&self, other: &dyn Parseable) -> bool {
        self.summarize() == other.summarize()
    }
    fn summarize(&self) -> String {
        match self {
            KObject::Number(n) => n.to_string(),
            KObject::KString(s) => s.clone(),
            KObject::Bool(b) => b.to_string(),
            KObject::List(items) => {
                let parts: Vec<String> = items.iter().map(|i| i.summarize()).collect();
                format!("[{}]", parts.join(", "))
            }
            KObject::Dict(entries) => {
                let parts: Vec<String> = entries
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k.summarize(), v.summarize()))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            KObject::KExpression(e) => e.summarize(),
            KObject::KFuture(t, _) => t.parsed.summarize(),
            KObject::KFunction(f, _) => f.summarize(),
            KObject::TaggedUnionType(schema) => {
                let parts: Vec<String> = schema
                    .iter()
                    .map(|(tag, ktype)| format!("{}: {}", tag, ktype.name()))
                    .collect();
                format!("Union{{{}}}", parts.join(", "))
            }
            KObject::Tagged { tag, value } => format!("{}({})", tag, value.summarize()),
            KObject::Null => "null".to_string(),
        }
    }
}
