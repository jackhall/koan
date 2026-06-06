//! Scope-bound resolution of a surface [`TypeName`] into an arena-allocated `&KType`.
//!
//! Read-only consumer of the bindings façade: never touches `data`, `functions`,
//! `placeholders`, `pending`, `out`, or `kind` — the read-only dependency is what
//! justifies the split from `scope.rs`.
//!
//! ## Invariant pinned here
//!
//! **The `type_expr_memo` is monotonic and never caches a not-yet-sealed type.**
//! An entry is written only on the `Done` arm AND only when every user-type the
//! elaborated result references is finalized (absent from its owning scope's
//! `pending_types`). The `Park` arm — a referenced type still in flight — never writes the
//! cache, so a half-built identity cannot leak into a later memo hit.

use crate::machine::core::kfunction::NodeId;
use crate::machine::core::{KError, KErrorKind, LexicalFrame, Scope, ScopeId};
use crate::machine::model::ast::TypeName;
use crate::machine::model::types::KType;
use crate::machine::model::values::KObject;

/// Outcome of [`Scope::resolve_type_expr`]. Mirrors
/// [`crate::machine::model::types::ElabResult`] but `Done` carries an
/// arena-allocated cache reference and `Park` carries scheduler `NodeId`s.
pub enum ResolveTypeExprOutcome<'a> {
    Done(&'a KType<'a>),
    Park(Vec<NodeId>),
    Unbound(String),
}

impl<'a> Scope<'a> {
    /// Layer-2 scope-bound TypeName resolution memo. On miss, runs
    /// [`crate::machine::model::types::elaborate_type_expr`] against `self`, asks a
    /// [`FinalizeGate`] whether the result is safe to share, and writes the cache
    /// only when the gate admits. The Park arm — elaborator-parked or gate-rejected —
    /// never writes the cache: caching mid-SCC would observe pre-close opaque identity.
    pub fn resolve_type_expr(
        &'a self,
        te: &TypeName,
        chain: Option<std::rc::Rc<LexicalFrame>>,
    ) -> ResolveTypeExprOutcome<'a> {
        use crate::machine::model::types::{elaborate_type_expr, ElabResult, Elaborator};
        // The cutoff this scope's bindings are gated against — also the memo key, so a
        // forward and a backward consumer never share a cached verdict.
        let cutoff = chain.as_ref().and_then(|c| c.index_for(self.id));
        if let Some(kt) = self.type_expr_memo_get(te, cutoff) {
            return ResolveTypeExprOutcome::Done(kt);
        }
        let mut elaborator = Elaborator::new(self).with_chain(chain);
        match elaborate_type_expr(&mut elaborator, te) {
            ElabResult::Done(kt) => {
                let pending = FinalizeGate { scope: self }.pending_producers(&kt);
                if pending.is_empty() {
                    let kt_ref: &'a KType<'a> = self.arena.alloc_ktype(kt);
                    self.type_expr_memo_insert(te.clone(), cutoff, kt_ref);
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

/// Resolve a bare leaf [`TypeName`] against `scope`'s type-side bindings and return the
/// canonical value-side `KObject` carrier.
///
/// - Parameterized shapes (`List<...>`, `Function<...>` etc.) are rejected with `ShapeError`.
/// - For a `SetRef` / `Module` identity, recover the paired value-side carrier when
///   present, so downstream operators see the original value rather than a synthesized
///   `KTypeValue`. No nominal binder dual-writes anymore (SIG was the last), so the
///   recovery typically misses and falls through to synthesis.
/// - Otherwise synthesize `KObject::KTypeValue(kt.clone())` so the value sits in the
///   same dispatch transport every other body consumes — this is how a struct / union /
///   module / Result / signature type token reaches a constructor or ATTR call site.
/// - Miss surfaces `UnboundName(name)`.
pub fn coerce_type_token_value<'a>(
    scope: &'a Scope<'a>,
    t: &TypeName,
    chain: Option<&LexicalFrame>,
) -> Result<&'a KObject<'a>, KError> {
    let name = t.as_str();
    match scope.resolve_type_with_chain(name, chain) {
        Some(kt) => {
            if matches!(kt, KType::SetRef { .. } | KType::Module { .. }) {
                if let Some(obj) = scope.lookup_with_chain(name, chain) {
                    return Ok(obj);
                }
                // Defensive fall-through when finalize skipped the paired-carrier install.
            }
            Ok(scope.arena.alloc_object(KObject::KTypeValue(kt.clone())))
        }
        None => Err(KError::new(KErrorKind::UnboundName(name.to_string()))),
    }
}

/// Precondition value for the `type_expr_memo` cache, naming the load-bearing
/// invariant *"no not-yet-sealed type may enter the memo"* as a type.
///
/// Admits a `KType` iff every top-level user-type it references is finalized in
/// its owning scope (absent from that scope's `pending_types`); otherwise returns
/// the producer `NodeId`s the caller parks on.
struct FinalizeGate<'a> {
    scope: &'a Scope<'a>,
}

impl<'a> FinalizeGate<'a> {
    /// Producer `NodeId`s the caller must park on; empty iff the gate admits.
    fn pending_producers(&self, kt: &KType<'_>) -> Vec<NodeId> {
        let mut pending: Vec<NodeId> = Vec::new();
        for (scope_id, name) in KTypeUserRefs::of(kt) {
            let Some(owner) = self.scope.ancestors().find(|s| s.id == scope_id) else {
                continue;
            };
            if !owner.bindings().pending_types().contains_key(name) {
                continue;
            }
            // `chain_cutoff = None` because this is producer-dependency tracking, not
            // consumer-visibility enforcement. A `Value`-arm hit would mean the
            // named type already finalized, which the `pending_types` check above
            // rules out for any name reaching this branch.
            if let Some(crate::machine::core::Resolution::Placeholder(node_id)) =
                owner.bindings().lookup_value(name, None)
            {
                if !pending.contains(&node_id) {
                    pending.push(node_id);
                }
            }
        }
        pending
    }
}

/// Iterator yielding every top-level user-type reference `(scope_id, name)` in a
/// `KType`.
///
/// **Set discipline** (load-bearing): a `SetRef` is a leaf — does NOT descend the
/// referenced member's schema, whose identity is `(set ptr, index)` and which may be
/// cyclic. The dependency a consumer parks on is the named binder itself; its schema's own
/// references are that binder's concern, resolved when it finalizes.
struct KTypeUserRefs<'k, 'a> {
    stack: Vec<&'k KType<'a>>,
}

impl<'k, 'a> KTypeUserRefs<'k, 'a> {
    fn of(kt: &'k KType<'a>) -> Self {
        Self { stack: vec![kt] }
    }
}

impl<'k, 'a> Iterator for KTypeUserRefs<'k, 'a> {
    type Item = (ScopeId, &'k str);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(kt) = self.stack.pop() {
            match kt {
                KType::SetRef { set, index } => {
                    let member = set.member(*index);
                    return Some((member.scope_id, member.name.as_str()));
                }
                KType::Signature { sig, .. } => {
                    return Some((sig.sig_id(), sig.path.as_str()));
                }
                KType::Module { module, .. } => {
                    return Some((module.scope_id(), module.path.as_str()));
                }
                KType::AbstractType { source, name } => {
                    return Some((source.scope_id(), name.as_str()));
                }
                KType::List(inner) => self.stack.push(inner),
                KType::Dict(k, v) => {
                    self.stack.push(v);
                    self.stack.push(k);
                }
                // Walk each field's type; `Record::values()` order is immaterial here.
                KType::Record(fields) => {
                    for t in fields.values() {
                        self.stack.push(t);
                    }
                }
                // Order is immaterial (the walker only collects the set of nested
                // user-type refs), and `Record::values()` is not double-ended, so no `.rev()`.
                KType::KFunction { params, ret } => {
                    self.stack.push(ret);
                    for a in params.values() {
                        self.stack.push(a);
                    }
                }
                KType::KFunctor { params, ret, .. } => {
                    self.stack.push(ret);
                    for p in params.values() {
                        self.stack.push(p);
                    }
                }
                KType::ConstructorApply { ctor, args } => {
                    for a in args.iter().rev() {
                        self.stack.push(a);
                    }
                    self.stack.push(ctor);
                }
                // Leaves / wildcards: no nested user‑type references at this level.
                // `DeferredReturn` carries only a hashable surface shadow (no nested
                // `KType`), so it bottoms out here.
                KType::Number
                | KType::Str
                | KType::Bool
                | KType::Null
                | KType::Identifier
                | KType::KExpression
                | KType::SigiledTypeExpr
                | KType::TypeExprRef
                | KType::Type
                | KType::AnyModule
                | KType::AnySignature
                | KType::Any
                | KType::AnyUserType { .. }
                | KType::DeferredReturn(_)
                | KType::SetLocal(_)
                | KType::RecursiveRef(_)
                | KType::RecursiveGroup(_) => {}
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

    #[test]
    fn resolve_type_expr_builtin_leaf_caches() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let te = TypeName::leaf("Number".into());
        let first = match scope.resolve_type_expr(&te, None) {
            ResolveTypeExprOutcome::Done(kt) => kt,
            _ => panic!("expected Done"),
        };
        assert_eq!(*first, KType::Number);
        let second = match scope.resolve_type_expr(&te, None) {
            ResolveTypeExprOutcome::Done(kt) => kt,
            _ => panic!("expected Done on second call"),
        };
        assert!(
            std::ptr::eq(first, second),
            "second call should hit the memo"
        );
    }

    #[test]
    fn resolve_type_expr_unbound_returns_unbound() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let te = TypeName::leaf("NotABuiltin".into());
        match scope.resolve_type_expr(&te, None) {
            ResolveTypeExprOutcome::Unbound(_) => {}
            _ => panic!("expected Unbound for unknown leaf"),
        }
    }

    /// Pins the post-finalize memo path: a user type reached after STRUCT
    /// finalize lands in the cache.
    #[test]
    fn resolve_type_expr_user_struct_caches_after_finalize() {
        use crate::builtins::test_support::{parse_one, run_one};
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run_one(scope, parse_one("STRUCT Point = (x :Number, y :Number)"));
        let te = TypeName::leaf("Point".into());
        let kt = match scope.resolve_type_expr(&te, None) {
            ResolveTypeExprOutcome::Done(kt) => kt,
            _ => panic!("expected Done after STRUCT declaration"),
        };
        match kt {
            KType::SetRef { set, index } => assert_eq!(set.member(*index).name, "Point"),
            _ => panic!("expected SetRef for Point"),
        }
        let kt2 = match scope.resolve_type_expr(&te, None) {
            ResolveTypeExprOutcome::Done(kt) => kt,
            _ => panic!("expected Done on memo hit"),
        };
        assert!(std::ptr::eq(kt, kt2));
    }

    /// A singleton struct `SetRef` named `name` at `scope_id`.
    fn struct_setref<'a>(name: &str, scope_id: ScopeId) -> KType<'a> {
        use crate::machine::model::types::{NominalSchema, RecursiveSet};
        use crate::machine::model::Record;
        let set =
            RecursiveSet::singleton(name.into(), scope_id, NominalSchema::Struct(Record::new()));
        KType::SetRef { set, index: 0 }
    }

    /// Pins recursion shape against a regression that skips nested structurals.
    #[test]
    fn ktype_user_refs_yields_nested_structural_refs_in_order() {
        let a_id = ScopeId::next();
        let b_id = ScopeId::next();
        let user_a = struct_setref("A", a_id);
        let user_b = struct_setref("B", b_id);
        // Dict<A, List<B>>
        let kt = KType::Dict(Box::new(user_a), Box::new(KType::List(Box::new(user_b))));
        let refs: Vec<(ScopeId, String)> = KTypeUserRefs::of(&kt)
            .map(|(id, n)| (id, n.to_string()))
            .collect();
        assert_eq!(refs, vec![(a_id, "A".into()), (b_id, "B".into())]);
    }

    /// SCC discipline: the iterator must not descend into a `SetRef` member's schema —
    /// the outer `SetRef` is yielded, the inner stays invisible.
    #[test]
    fn ktype_user_refs_does_not_recurse_into_user_type_payload() {
        use crate::machine::model::types::{NominalSchema, RecursiveSet};
        let outer_id = ScopeId::next();
        let inner_id = ScopeId::next();
        let inner = struct_setref("Inner", inner_id);
        let outer = {
            let set = RecursiveSet::singleton(
                "Outer".into(),
                outer_id,
                NominalSchema::Newtype(Box::new(inner)),
            );
            KType::SetRef { set, index: 0 }
        };
        let refs: Vec<(ScopeId, String)> = KTypeUserRefs::of(&outer)
            .map(|(id, n)| (id, n.to_string()))
            .collect();
        assert_eq!(refs, vec![(outer_id, "Outer".into())]);
    }

    /// Pin against a regression that would push a spurious leaf onto the stack.
    #[test]
    fn ktype_user_refs_yields_nothing_for_leaf() {
        let mut iter = KTypeUserRefs::of(&KType::Number);
        assert!(iter.next().is_none());
    }

    mod coerce_type_token_value {
        use super::super::coerce_type_token_value;
        use crate::builtins::test_support::run_root_bare;
        use crate::machine::core::{BindingIndex, ScopeId};
        use crate::machine::model::ast::TypeName;
        use crate::machine::model::{KObject, KType};
        use crate::machine::{KError, KErrorKind, RuntimeArena};

        #[test]
        fn builtin_synthesizes_ktypevalue() {
            let arena = RuntimeArena::new();
            let scope = run_root_bare(&arena);
            scope.register_type("Number".into(), KType::Number, BindingIndex::BUILTIN);
            let leaf = TypeName::leaf("Number".to_string());
            let obj = coerce_type_token_value(scope, &leaf, None).expect("expected Number lookup");
            assert!(matches!(obj, KObject::KTypeValue(KType::Number)));
        }

        #[test]
        fn unbound_returns_error() {
            let arena = RuntimeArena::new();
            let scope = run_root_bare(&arena);
            let leaf = TypeName::leaf("Missing".to_string());
            match coerce_type_token_value(scope, &leaf, None) {
                Err(KError {
                    kind: KErrorKind::UnboundName(name),
                    ..
                }) => {
                    assert_eq!(name, "Missing");
                }
                other => panic!("expected UnboundName, got {:?}", other.map(|_| "Ok(_)")),
            }
        }

        /// A singleton struct `SetRef` named `name` at `scope_id`.
        fn struct_setref<'a>(name: &str, scope_id: ScopeId) -> KType<'a> {
            use crate::machine::model::types::{NominalSchema, RecursiveSet};
            use crate::machine::model::Record;
            let set = RecursiveSet::singleton(
                name.into(),
                scope_id,
                NominalSchema::Struct(Record::new()),
            );
            KType::SetRef { set, index: 0 }
        }

        #[test]
        fn recovers_paired_value() {
            let arena = RuntimeArena::new();
            let scope = run_root_bare(&arena);
            let kt = struct_setref("Point", scope.id);
            scope.register_type("Point".into(), kt.clone(), BindingIndex::BUILTIN);
            let paired = arena.alloc_object(KObject::KTypeValue(kt));
            scope
                .bind_value("Point".to_string(), paired, BindingIndex::BUILTIN)
                .unwrap();

            let leaf = TypeName::leaf("Point".to_string());
            let obj = coerce_type_token_value(scope, &leaf, None).expect("expected Point lookup");
            assert!(std::ptr::eq(obj, paired));
        }

        /// Defensive paired-recovery fall-through: when `bindings.types[name]` holds a
        /// nominal identity but `bindings.data[name]` is empty, the helper must not
        /// panic — it synthesizes a fresh `KTypeValue(kt)` so the dispatch transport
        /// stays valid. Unreachable in normal flow (nominal binders install both atomically).
        #[test]
        fn falls_through_when_paired_value_absent() {
            let arena = RuntimeArena::new();
            let scope = run_root_bare(&arena);
            let kt = struct_setref("Orphan", scope.id);
            // types-side only — no paired `bind_value`.
            scope.register_type("Orphan".into(), kt.clone(), BindingIndex::BUILTIN);

            let leaf = TypeName::leaf("Orphan".to_string());
            let obj = coerce_type_token_value(scope, &leaf, None).expect("fall-through must Ok");
            match obj {
                KObject::KTypeValue(KType::SetRef { set, index }) => {
                    assert_eq!(set.member(*index).name, "Orphan");
                }
                other => panic!(
                    "expected synthesized KTypeValue(SetRef(Orphan)), got {:?}",
                    other.ktype()
                ),
            }
        }
    }
}
