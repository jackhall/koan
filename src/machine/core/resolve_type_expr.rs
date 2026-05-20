//! Scope-bound resolution of a surface [`TypeExpr`] into an arena-allocated `&KType`.
//!
//! Read-only consumer of the bindings façade: takes a [`TypeExpr`], runs the elaborator
//! against the scope chain, and caches the result into `bindings.type_expr_memo` once
//! the result is safe to share. Never touches `data`, `functions`, `placeholders`,
//! `pending`, `out`, or `kind` — the read‑only dependency is what justifies the split
//! from `scope.rs`.
//!
//! ## Invariant pinned here
//!
//! **The `type_expr_memo` is monotonic and never observes pre‑close opaque identity.**
//! An entry is written only on the `Done` arm AND only when every user‑type the
//! elaborated result references is finalized (i.e. absent from its owning scope's
//! `pending_types`). The `Park` arm — whether from the elaborator itself or from a
//! still-pending user-type referenced by the elaborated result — never writes the
//! cache, so a mid‑SCC pre-close `UserType` identity cannot leak into a later memo
//! hit.
//!
//! ## SCC awareness
//!
//! [`KTypeUserRefs`] walks the *top level* of a `KType` only — it does NOT recurse
//! into a `UserType`'s `kind` payload. SCC closure is atomic across all members of
//! a cycle, so a finalized `Foo` guarantees every user‑type embedded in `Foo`'s
//! payload is also finalized; descending into payloads would only re‑prove that.

use crate::machine::core::kfunction::NodeId;
use crate::machine::model::ast::TypeExpr;
use crate::machine::model::types::KType;

use super::scope::Scope;
use super::scope_id::ScopeId;

/// Outcome of [`Scope::resolve_type_expr`]. The three variants mirror
/// [`crate::machine::model::types::ElabResult`] but `Done` carries the
/// arena-allocated cache reference and `Park` carries scheduler `NodeId`s the
/// caller parks on.
pub enum ResolveTypeExprOutcome<'a> {
    Done(&'a KType),
    Park(Vec<NodeId>),
    Unbound(String),
}

impl<'a> Scope<'a> {
    /// Layer‑2 scope‑bound TypeExpr resolution memo. Tries the cache first; on miss,
    /// runs [`crate::machine::model::types::elaborate_type_expr`] against `self`,
    /// asks a [`FinalizeGate`] whether the elaborated result is safe to share, and
    /// writes back into the cache only when the gate admits it.
    ///
    /// Three outcomes:
    /// - `Done(kt)`: elaboration succeeded and the gate admitted (every referenced
    ///   user‑type is finalized). The result is arena‑allocated and cached.
    /// - `Park(producers)`: the elaborator parked on a placeholder, OR the gate
    ///   rejected (the elaborated result references a still‑pending user‑type).
    ///   The producer `NodeId`s identify the wake sources the caller parks on.
    ///   **No cache write** in the Park case — caching mid‑SCC‑window would
    ///   observe pre‑close opaque identity.
    /// - `Unbound(msg)`: a leaf name didn't resolve anywhere. Caller wraps in a
    ///   structured `ShapeError`.
    pub fn resolve_type_expr(&'a self, te: &TypeExpr) -> ResolveTypeExprOutcome<'a> {
        use crate::machine::model::types::{elaborate_type_expr, ElabResult, Elaborator};
        if let Some(kt) = self.type_expr_memo_get(te) {
            return ResolveTypeExprOutcome::Done(kt);
        }
        let mut elaborator = Elaborator::new(self);
        match elaborate_type_expr(&mut elaborator, te) {
            ElabResult::Done(kt) => {
                let pending = FinalizeGate { scope: self }.pending_producers(&kt);
                if pending.is_empty() {
                    let kt_ref: &'a KType = self.arena.alloc_ktype(kt);
                    self.type_expr_memo_insert(te.clone(), kt_ref);
                    ResolveTypeExprOutcome::Done(kt_ref)
                } else {
                    ResolveTypeExprOutcome::Park(pending)
                }
            }
            ElabResult::Park(producers) => ResolveTypeExprOutcome::Park(producers),
            ElabResult::Unbound(msg) => ResolveTypeExprOutcome::Unbound(msg),
        }
    }
}

/// Precondition value for the `type_expr_memo` cache: names the load‑bearing
/// invariant *"no pre‑close user‑type identity may enter the memo"* as a type
/// rather than a free function. Constructed at the point of decision (post
/// `ElabResult::Done`) and consulted once.
///
/// The gate **admits** a `KType` iff every top‑level user‑type it references is
/// finalized in its owning scope (absent from that scope's `pending_types`);
/// otherwise it returns the producer `NodeId`s the caller parks on. Held by
/// reference to `Scope` — no state, just a named decision point.
struct FinalizeGate<'a> {
    scope: &'a Scope<'a>,
}

impl<'a> FinalizeGate<'a> {
    /// Producer `NodeId`s the caller must park on; empty iff the gate admits.
    /// Walks the *top level* of `kt` via [`KTypeUserRefs`], looks each
    /// `(scope_id, name)` up in its owning scope's `pending_types`, and
    /// deduplicates the producer ids it collects.
    fn pending_producers(&self, kt: &KType) -> Vec<NodeId> {
        let mut pending: Vec<NodeId> = Vec::new();
        for (scope_id, name) in KTypeUserRefs::of(kt) {
            let Some(owner) = self.scope.ancestors().find(|s| s.id == scope_id) else {
                continue;
            };
            if !owner.bindings().pending_types().contains_key(name) {
                continue;
            }
            if let Some(node_id) = owner.bindings().placeholders().get(name).copied() {
                if !pending.contains(&node_id) {
                    pending.push(node_id);
                }
            }
        }
        pending
    }
}

/// Iterator yielding every top‑level user‑type reference `(scope_id, name)` in a
/// `KType`. Replaces the previous `referenced_user_types(kt) -> Vec<(ScopeId,
/// String)>` collector — borrows the name as `&str` rather than cloning, and
/// keeps a small `&KType` stack as its only state.
///
/// **SCC discipline** (load‑bearing): does NOT recurse into a `UserType`'s
/// `kind` payload. SCC closure is atomic across members, so a finalized `Foo`
/// guarantees every user‑type embedded in `Foo`'s payload is also finalized;
/// payload recursion would only re‑prove that.
struct KTypeUserRefs<'k> {
    stack: Vec<&'k KType>,
}

impl<'k> KTypeUserRefs<'k> {
    fn of(kt: &'k KType) -> Self {
        Self { stack: vec![kt] }
    }
}

impl<'k> Iterator for KTypeUserRefs<'k> {
    type Item = (ScopeId, &'k str);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(kt) = self.stack.pop() {
            match kt {
                KType::UserType { scope_id, name, .. } => {
                    return Some((*scope_id, name.as_str()));
                }
                KType::SignatureBound { sig_id, sig_path, .. } => {
                    return Some((*sig_id, sig_path.as_str()));
                }
                KType::List(inner) => self.stack.push(inner),
                KType::Dict(k, v) => {
                    self.stack.push(v);
                    self.stack.push(k);
                }
                KType::KFunction { args, ret } => {
                    self.stack.push(ret);
                    for a in args.iter().rev() {
                        self.stack.push(a);
                    }
                }
                KType::Mu { body, .. } => self.stack.push(body),
                KType::ConstructorApply { ctor, args } => {
                    for a in args.iter().rev() {
                        self.stack.push(a);
                    }
                    self.stack.push(ctor);
                }
                // Leaves / wildcards: no nested user‑type references at this level.
                KType::Number
                | KType::Str
                | KType::Bool
                | KType::Null
                | KType::Identifier
                | KType::KExpression
                | KType::TypeExprRef
                | KType::Type
                | KType::Signature
                | KType::Any
                | KType::AnyUserType { .. }
                | KType::RecursiveRef(_) => {}
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::test_support::run_root_silent;
    use crate::machine::core::RuntimeArena;

    /// Builtin leaf resolves to `Done` and caches the result; the second call returns
    /// the same arena pointer without re-walking.
    #[test]
    fn resolve_type_expr_builtin_leaf_caches() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let te = TypeExpr::leaf("Number".into());
        let first = match scope.resolve_type_expr(&te) {
            ResolveTypeExprOutcome::Done(kt) => kt,
            _ => panic!("expected Done"),
        };
        assert_eq!(*first, KType::Number);
        let second = match scope.resolve_type_expr(&te) {
            ResolveTypeExprOutcome::Done(kt) => kt,
            _ => panic!("expected Done on second call"),
        };
        assert!(std::ptr::eq(first, second), "second call should hit the memo");
    }

    /// A genuinely unbound leaf surfaces as `Unbound` and is not cached.
    #[test]
    fn resolve_type_expr_unbound_returns_unbound() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let te = TypeExpr::leaf("NotABuiltin".into());
        match scope.resolve_type_expr(&te) {
            ResolveTypeExprOutcome::Unbound(_) => {}
            _ => panic!("expected Unbound for unknown leaf"),
        }
    }

    /// User-bound types reached after finalize land in the memo. Build a STRUCT,
    /// then resolve a TypeExpr for its name — should `Done` and cache.
    #[test]
    fn resolve_type_expr_user_struct_caches_after_finalize() {
        use crate::builtins::test_support::{parse_one, run_one};
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run_one(scope, parse_one("STRUCT Point = (x :Number, y :Number)"));
        let te = TypeExpr::leaf("Point".into());
        let kt = match scope.resolve_type_expr(&te) {
            ResolveTypeExprOutcome::Done(kt) => kt,
            _ => panic!("expected Done after STRUCT declaration"),
        };
        match kt {
            KType::UserType { name, .. } => assert_eq!(name, "Point"),
            _ => panic!("expected UserType for Point"),
        }
        // Second resolve hits the memo and returns the same pointer.
        let kt2 = match scope.resolve_type_expr(&te) {
            ResolveTypeExprOutcome::Done(kt) => kt,
            _ => panic!("expected Done on memo hit"),
        };
        assert!(std::ptr::eq(kt, kt2));
    }

    /// `KTypeUserRefs` walks structural variants (`List`, `Dict`, `KFunction`,
    /// etc.) and yields top‑level user‑type refs in source order. Pins the
    /// recursion shape against a regression that would skip nested structurals.
    #[test]
    fn ktype_user_refs_yields_nested_structural_refs_in_order() {
        use crate::machine::model::types::UserTypeKind;
        let a_id = ScopeId::next();
        let b_id = ScopeId::next();
        let user_a = KType::UserType {
            kind: UserTypeKind::Struct,
            scope_id: a_id,
            name: "A".into(),
        };
        let user_b = KType::UserType {
            kind: UserTypeKind::Struct,
            scope_id: b_id,
            name: "B".into(),
        };
        // Dict<A, List<B>> — A is the key, B is nested inside the value's List.
        let kt = KType::Dict(Box::new(user_a), Box::new(KType::List(Box::new(user_b))));
        let refs: Vec<(ScopeId, String)> =
            KTypeUserRefs::of(&kt).map(|(id, n)| (id, n.to_string())).collect();
        assert_eq!(refs, vec![(a_id, "A".into()), (b_id, "B".into())]);
    }

    /// SCC discipline: a `UserType` with a payload that itself contains another
    /// `UserType` (e.g. `NEWTYPE` `repr`) must NOT be recursed into. The outer
    /// `UserType` is yielded; the inner is invisible to the iterator. Guards
    /// against a "fix" that accidentally descends into `UserType.kind`.
    #[test]
    fn ktype_user_refs_does_not_recurse_into_user_type_payload() {
        use crate::machine::model::types::UserTypeKind;
        let outer_id = ScopeId::next();
        let inner_id = ScopeId::next();
        let inner = KType::UserType {
            kind: UserTypeKind::Struct,
            scope_id: inner_id,
            name: "Inner".into(),
        };
        let outer = KType::UserType {
            kind: UserTypeKind::Newtype { repr: Box::new(inner) },
            scope_id: outer_id,
            name: "Outer".into(),
        };
        let refs: Vec<(ScopeId, String)> =
            KTypeUserRefs::of(&outer).map(|(id, n)| (id, n.to_string())).collect();
        assert_eq!(refs, vec![(outer_id, "Outer".into())]);
    }

    /// Leaf types (`Number`, `Str`, etc.) yield nothing — the iterator drains
    /// to `None` immediately. Cheap pin against a regression that would push a
    /// spurious leaf onto the stack.
    #[test]
    fn ktype_user_refs_yields_nothing_for_leaf() {
        let mut iter = KTypeUserRefs::of(&KType::Number);
        assert!(iter.next().is_none());
    }
}
