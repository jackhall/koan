//! `Module` and `Signature` — first-class module values produced by the `MODULE` and `SIG`
//! builtins. See [design/typing/modules.md](../../../../design/typing/modules.md).
//!
//! **Terminology — "module-signature" vs "expression-signature".** `Signature` here is the
//! **module-signature** type (`SIG`-declared) — an interface a module can be ascribed to
//! via `:|` / `:!`. The **expression-signature** machinery (`ExpressionSignature`,
//! `Argument`, `SignatureElement`) lives in [`crate::machine::model::types::signature`]. The two
//! are distinct concepts; do not conflate.
//!
//! Lifetime erasure on the scope pointer routes through
//! [`ScopePtr`](crate::machine::core::scope_ptr::ScopePtr), shared with
//! [`KFunction`](crate::machine::core::kfunction::KFunction) and
//! [`CallArena`](crate::machine::core::arena::CallArena). The branded `ScopePtr<'a>` makes
//! `child_scope` / `decl_scope` safe re-attaches; the irreducible `unsafe` re-attach lives at
//! `CallArena`.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::machine::core::{Scope, ScopeId, ScopePtr};

use super::super::types::KType;

/// First-class module value. `path` is the lexical-source label (`"IntOrd"`,
/// `"Outer.Inner"`); `type_members` maps the module's abstract type names to the `KType`
/// they currently expose. Opaque-ascription members mint `KType::AbstractType { source:
/// Module(self), name }`; the module value itself rides `KType::Module { module, frame }`
/// in the surrounding `KObject::KTypeValue` (the two are distinguished by KType variant,
/// not by a shared `UserType` `kind` tag).
pub struct Module<'a> {
    pub path: String,
    child_scope_ptr: ScopePtr<'a>,
    /// `RefCell` because opaque-ascription installs entries after the surrounding `KObject`
    /// is alloc'd. `Module` is arena-pinned and never moved, so a `&'a Module<'a>` borrow
    /// stays valid alongside interior mutation.
    pub type_members: RefCell<HashMap<String, KType<'a>>>,
    /// VAL-slot name → the per-call abstract `KType` an opaque ascription minted for the
    /// slot's SIG-declared type. ATTR re-tags a value-side slot read with this identity so
    /// `(int_ord.zero)` reads as the abstract `Type`, not the underlying concrete value.
    /// Empty for unascribed and transparently-ascribed (`:!`) modules. Same `RefCell`
    /// rationale as `type_members` — populated after the surrounding `KObject` is alloc'd.
    pub slot_type_tags: RefCell<HashMap<String, KType<'a>>>,
    /// Sigs this module shape-checks against. `accepts_part` for a
    /// `KType::Signature { sig, .. }` slot is an O(1) `sig.sig_id()` membership check
    /// against this set. `RefCell` for the same reason as `type_members` — ascription
    /// writes after the surrounding `KObject::KModule` is already alloc'd.
    pub compatible_sigs: RefCell<Vec<ScopeId>>,
}

impl<'a> Module<'a> {
    pub fn new(path: String, child_scope: &'a Scope<'a>) -> Self {
        Self {
            path,
            child_scope_ptr: ScopePtr::erase(child_scope),
            type_members: RefCell::new(HashMap::new()),
            slot_type_tags: RefCell::new(HashMap::new()),
            compatible_sigs: RefCell::new(Vec::new()),
        }
    }

    /// Record that this module shape-checks against `sig_id`. Idempotent — re-ascribing
    /// (e.g. `(View :| OrderedSig)` after `(View :! OrderedSig)`) doesn't double-insert.
    pub fn mark_satisfies(&self, sig_id: ScopeId) {
        let mut s = self.compatible_sigs.borrow_mut();
        if !s.contains(&sig_id) {
            s.push(sig_id);
        }
    }

    /// Re-attach `'a` to the stored scope. The branded `child_scope_ptr` makes this a safe
    /// re-attach: it consumed a real `&'a Scope<'a>` at construction, and the arena outlives
    /// every `&Module<'a>` by construction.
    pub fn child_scope(&self) -> &'a Scope<'a> {
        self.child_scope_ptr.reattach()
    }

    /// Stable identity keyed by `KType::Module` equality (and recorded on per-call abstract
    /// members minted from this module). Two distinct opaque ascriptions of the same source
    /// module compare distinct because each allocates a fresh child scope (and thus a fresh
    /// `ScopeId`).
    pub fn scope_id(&self) -> ScopeId {
        self.child_scope().id
    }
}

/// First-class signature (module type) value. Holds the raw declaration scope so
/// `:|` / `:!` can iterate the declared abstract types and operation signatures at
/// ascription time.
pub struct Signature<'a> {
    pub path: String,
    /// Branded [`ScopePtr<'a>`]: `Scope<'a>` is invariant in `'a`, and the brand's
    /// `PhantomData<&'a Scope<'a>>` carries that invariance structurally, so it is what pins
    /// `Signature<'a>` invariant in `'a` — no separate marker field is needed.
    decl_scope_ptr: ScopePtr<'a>,
}

impl<'a> Signature<'a> {
    pub fn new(path: String, decl_scope: &'a Scope<'a>) -> Self {
        Self {
            path,
            decl_scope_ptr: ScopePtr::erase(decl_scope),
        }
    }

    /// Re-attach `'a` to the stored scope. The branded `decl_scope_ptr` makes this a safe
    /// re-attach: the decl scope is arena-allocated and outlives every `&Signature<'a>` by
    /// construction.
    pub fn decl_scope(&self) -> &'a Scope<'a> {
        self.decl_scope_ptr.reattach()
    }

    /// Stable identity for `KType::Signature { sig, .. }` (its dispatch identity is
    /// `sig.sig_id()` + `pinned_slots`). Each `SIG` declares its own decl_scope and thus a
    /// fresh `ScopeId`; two `SIG Foo = (...)` in the same lexical scope already error
    /// (`Rebind`), so distinct `Signature`s always have distinct ids.
    pub fn sig_id(&self) -> ScopeId {
        self.decl_scope().id
    }
}

#[cfg(test)]
mod tests {
    //! Miri coverage for the unsafe sites: `*const Scope<'static>` lifetime-erasure
    //! transmutes and `type_members` `RefCell` mutation under a held `&'a Module<'a>`
    //! borrow. Each shape is exercised in isolation so a regression attributes to a
    //! single site. See [`design/memory-model.md`](../../../../design/memory-model.md).
    use super::*;
    use crate::builtins::default_scope;
    use crate::machine::core::RuntimeArena;
    use crate::machine::model::types::{AbstractSource, KType};
    use std::io::sink;
    use std::ptr;
    #[test]
    fn module_child_scope_transmute_does_not_dangle() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(sink()));
        let module = arena.alloc_module(Module::new("Test".into(), scope));
        let recovered = module.child_scope();
        assert!(ptr::eq(recovered, scope));
        // Re-borrow after a sibling alloc — tree borrows is sensitive to interleaved
        // mutation under live shared borrows.
        let _other = arena.alloc_object(crate::machine::model::values::KObject::Number(1.0));
        let recovered2 = module.child_scope();
        assert!(ptr::eq(recovered2, scope));
    }

    /// Covered independently of the module path because `Signature` lives on a different
    /// sub-arena (`signatures`) — a regression in `alloc_signature` or `decl_scope` must
    /// surface without the module path masking it.
    #[test]
    fn signature_decl_scope_transmute_does_not_dangle() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(sink()));
        let sig = arena.alloc_signature(Signature::new("OrderedSig".into(), scope));
        let recovered = sig.decl_scope();
        assert!(ptr::eq(recovered, scope));
        let _other = arena.alloc_object(crate::machine::model::values::KObject::Number(1.0));
        let recovered2 = sig.decl_scope();
        assert!(ptr::eq(recovered2, scope));
    }

    /// Opaque ascription mutates `type_members` after the surrounding `KObject` is alloc'd,
    /// so the `&'a Module<'a>` borrow is live across the `borrow_mut` + insert. Tree
    /// borrows is strict about interior mutation under a live shared borrow.
    #[test]
    fn module_type_members_refcell_mutation_with_held_module_ref() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(sink()));
        let module = arena.alloc_module(Module::new("M".into(), scope));
        let scope_id = module.scope_id();
        {
            let mut tm = module.type_members.borrow_mut();
            tm.insert(
                "Type".into(),
                KType::AbstractType {
                    source: AbstractSource::Module(module),
                    name: "Type".into(),
                },
            );
        }
        let bound = module.type_members.borrow().get("Type").cloned();
        assert!(matches!(
            &bound,
            Some(KType::AbstractType { source, name })
                if source.scope_id() == scope_id && name == "Type"
        ));
    }

    /// `slot_type_tags` mutates after the surrounding `KObject` is alloc'd, same as
    /// `type_members`: the `&'a Module<'a>` borrow is live across the `borrow_mut` +
    /// insert, and tree borrows is strict about interior mutation under a live shared
    /// borrow. Pinned independently so a regression attributes to this map's site.
    #[test]
    fn module_slot_type_tags_refcell_mutation_with_held_module_ref() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(sink()));
        let module = arena.alloc_module(Module::new("M".into(), scope));
        let scope_id = module.scope_id();
        {
            let mut tags = module.slot_type_tags.borrow_mut();
            tags.insert(
                "zero".into(),
                KType::AbstractType {
                    source: AbstractSource::Module(module),
                    name: "Type".into(),
                },
            );
        }
        let bound = module.slot_type_tags.borrow().get("zero").cloned();
        assert!(matches!(
            &bound,
            Some(KType::AbstractType { source, name })
                if source.scope_id() == scope_id && name == "Type"
        ));
    }

    /// Build a `KTypeValue(KType::Module { module, frame })` whose `child_scope` lives in
    /// a `CallArena`, lift it against the dying frame, and assert the lifted carrier
    /// carries the arena anchor. Pins the unsafe site behind functor execution end-to-end.
    #[test]
    fn functor_per_call_module_lifts_correctly() {
        use crate::machine::core::kfunction::{Body, KFunction};
        use crate::machine::core::{CallArena, RuntimeArena as RA};
        use crate::machine::execute::lift_kobject_for_test;
        use crate::machine::model::types::{
            ExpressionSignature, KType, ReturnType, SignatureElement,
        };
        use crate::machine::model::values::KObject;
        use std::rc::Rc;

        let outer_arena = RuntimeArena::new();
        let outer_scope = default_scope(&outer_arena, Box::new(sink()));
        let frame: Rc<CallArena> = CallArena::new(outer_scope, None);

        // Borrow into the per-call arena via raw-pointer roundtrip so the borrow doesn't
        // outlive `frame` for the borrow-checker (the SAFETY invariant on `CallArena` —
        // arena heap address is stable for the Rc's life — backs this).
        let arena_ptr: *const RA = frame.arena();
        let inner_arena: &RA = unsafe { &*arena_ptr };

        // Defeat `functions_is_empty()`'s fast path so the slow lift path runs.
        let kf = KFunction::new(
            ExpressionSignature {
                return_type: ReturnType::Resolved(KType::Null),
                elements: vec![SignatureElement::Keyword("__SLOW__".into())],
            },
            Body::Builtin(|s, _, _| {
                crate::machine::core::kfunction::BodyResult::Value(
                    s.arena.alloc_object(KObject::Null),
                )
            }),
            frame.scope(),
        );
        let _ = inner_arena.alloc_function(kf);

        // Module's `child_scope` lives in `inner_arena` — exactly the shape a functor
        // body's `MODULE Result = (...)` produces. Lift must observe the arena match.
        let inner_scope = inner_arena.alloc_scope(crate::machine::core::Scope::child_under_module(
            frame.scope(),
            "Inner".into(),
        ));
        let module = inner_arena.alloc_module(Module::new("Inner".into(), inner_scope));
        let m_obj = KObject::KTypeValue(KType::Module {
            module,
            frame: None,
        });

        let strong_before = Rc::strong_count(&frame);
        let lifted = lift_kobject_for_test(&m_obj, &frame);
        match &lifted {
            KObject::KTypeValue(KType::Module { frame: anchor, .. }) => assert!(
                anchor.is_some(),
                "Module carrier whose child scope lives in the dying arena must lift with frame=Some(rc)",
            ),
            other => panic!("expected lifted Module carrier, got {:?}", other.ktype()),
        }
        assert_eq!(
            Rc::strong_count(&frame),
            strong_before + 1,
            "lifting a per-frame module must clone the dying frame's Rc once",
        );
        // Drop borrowers before `frame` so arena teardown order is well-defined.
        drop(lifted);
        drop(m_obj);
    }
}
