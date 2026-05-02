use std::collections::HashMap;
use std::rc::Rc;

use crate::parse::kexpression::KExpression;
use super::arena::CallArena;
use super::ktraits::{Parseable, Serializable};
use super::kfunction::KFunction;
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
pub enum KObject<'a> {
    UserDefined,
    Number(f64),
    KString(String),
    Bool(bool),
    List(Rc<Vec<KObject<'a>>>),
    Dict(Rc<HashMap<Box<dyn Serializable + 'a>, KObject<'a>>>),
    KExpression(KExpression<'a>),
    KFuture(KFuture<'a>, Option<Rc<CallArena>>),
    KFunction(&'a KFunction<'a>, Option<Rc<CallArena>>),
    Null,
}

impl<'a> KObject<'a> {
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
            KObject::UserDefined => KObject::UserDefined,
            KObject::List(items) => KObject::List(Rc::clone(items)),
            KObject::Dict(entries) => KObject::Dict(Rc::clone(entries)),
            KObject::KExpression(e) => KObject::KExpression(e.clone()),
            KObject::KFuture(t, frame) => KObject::KFuture(t.deep_clone(), frame.clone()),
            KObject::KFunction(f, frame) => KObject::KFunction(*f, frame.clone()),
        }
    }
}

impl<'a> Parseable for KObject<'a> {
    fn equal(&self, other: &dyn Parseable) -> bool {
        self.summarize() == other.summarize()
    }
    fn summarize(&self) -> String {
        match self {
            KObject::UserDefined => "null".to_string(),
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
            KObject::Null => "null".to_string(),
        }
    }
}
