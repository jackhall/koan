//! The lexical binding façade: four co-mutating `RefCell` maps (`types`, `data`,
//! `functions`, `placeholders`) plus the shared validated write paths
//! ([`Bindings::try_apply`] for `data`/`functions`, [`Bindings::try_apply_type`] for
//! `types`) that keep the dual-map invariant — every `data[name]` entry wrapping a
//! `KFunction` lives in `functions[signature.untyped_key()]` — in one place. The
//! `types` map (added in stage 1.2 of per-type identity) is the dedicated home for
//! type-side bindings; it is currently unused (stage 1.4 wires `Scope::register_type`
//! into it). Borrow discipline across the four maps: `types → functions → data`,
//! with `types` only acquired when writing types. Lives in its own module so the
//! façade's surface (write methods + read handles) stays focused; [`Scope`] embeds it
//! by value so all interior borrows arbitrate against one another.
//!
//! `ApplyOutcome` is `pub` so [`scope`](super::scope)'s match arms on the
//! [`Bindings::try_bind_value`] / [`Bindings::try_register_function`] results compile, but
//! it isn't re-exported beyond `core::` — the `Conflict` variant is an internal queueing
//! signal, not part of any user-visible API.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;

use crate::runtime::machine::kfunction::{KFunction, NodeId};
use crate::runtime::model::types::{KType, UntypedKey};
use crate::runtime::model::values::KObject;

use super::kerror::{KError, KErrorKind};

/// Façade owning the four co-mutating `RefCell` maps that back every lexical binding:
/// `types` (name → `&KType`, the dedicated type-binding home introduced in stage 1.2 of
/// per-type identity; no consumer reads it yet — wiring lands in stage 1.4),
/// `data` (name → value), `functions` (untyped-signature bucket → overloads), and
/// `placeholders` (name → producer NodeId for dispatch-time forward references).
///
/// The shared private [`Bindings::try_apply`] enforces the dual-map invariant —
/// every `data[name]` entry wrapping a `KFunction` lives in
/// `functions[signature.untyped_key()]` — in one place, and unifies dedupe (`ptr::eq`
/// fast-path then `signatures_exact_equal`) across the LET-binds-FN and `FN`-decl paths.
/// [`Bindings::try_apply_type`] is the parallel write primitive for the `types` map; it
/// borrows only `types`, so it composes cleanly with code holding live `data`/`functions`
/// borrows.
///
/// Borrow discipline across the four maps: `types → functions → data`. `types` is only
/// acquired by the type-side write path, so the existing `functions → data` ordering used
/// by [`Bindings::try_apply`] is unaffected.
///
/// Lifetime `'a` matches the arena lifetime of the stored references; the façade itself
/// is embedded by value on [`super::scope::Scope`] so all interior borrows arbitrate
/// against one another.
pub struct Bindings<'a> {
    types: RefCell<HashMap<String, &'a KType>>,
    data: RefCell<HashMap<String, &'a KObject<'a>>>,
    functions: RefCell<HashMap<UntypedKey, Vec<&'a KFunction<'a>>>>,
    placeholders: RefCell<HashMap<String, NodeId>>,
}

impl<'a> Bindings<'a> {
    pub fn new() -> Self {
        Self {
            types: RefCell::new(HashMap::new()),
            data: RefCell::new(HashMap::new()),
            functions: RefCell::new(HashMap::new()),
            placeholders: RefCell::new(HashMap::new()),
        }
    }

    /// Read-only handle for the ~12 read sites in builtins and resolver code. The returned
    /// `Ref<'_, _>` has the lifetime of `&self`, so the usual `for (k, v) in
    /// scope.bindings().data().iter()` pattern extends the temporary through the loop —
    /// same semantics as the prior `RefCell::borrow()` calls.
    pub fn data(&self) -> Ref<'_, HashMap<String, &'a KObject<'a>>> {
        self.data.borrow()
    }

    /// Read-only handle for `resolve_dispatch`'s outer-chain walk and the submission-time
    /// `pre_run` extractor. Same `Ref<'_, _>` semantics as [`Bindings::data`].
    pub fn functions(&self) -> Ref<'_, HashMap<UntypedKey, Vec<&'a KFunction<'a>>>> {
        self.functions.borrow()
    }

    /// Read-only handle for the resolver's placeholder lookup. Same semantics as
    /// [`Bindings::data`].
    pub fn placeholders(&self) -> Ref<'_, HashMap<String, NodeId>> {
        self.placeholders.borrow()
    }

    /// Read-only handle for the type-side resolver path landing in stage 1.4
    /// (`Scope::resolve_type`) and the eventual type-class consumer migration.
    /// Same `Ref<'_, _>` semantics as [`Bindings::data`].
    pub fn types(&self) -> Ref<'_, HashMap<String, &'a KType>> {
        self.types.borrow()
    }

    /// LET-style value bind. Errors `Rebind` if `data[name]` already exists. When `obj`
    /// wraps a `KFunction`, the function is *also* mirrored into the `functions` bucket
    /// keyed by its untyped signature so dispatch finds it — supports `LET f = (FN ...)`
    /// where the bound name doubles as a callable verb.
    ///
    /// `Conflict` outcome means borrow contention (caller queues); `Err` is semantic
    /// rejection (not queued).
    pub fn try_bind_value(
        &self,
        name: &str,
        obj: &'a KObject<'a>,
    ) -> Result<ApplyOutcome, KError> {
        let fn_part = match obj {
            KObject::KFunction(f, _) => Some(*f),
            _ => None,
        };
        self.try_apply(name, obj, fn_part)
    }

    /// FN-style overload registration. Adds `fn_ref` to the `functions` bucket keyed by
    /// its untyped signature, then inserts `obj` into `data[name]`. Errors:
    /// - `DuplicateOverload` if the bucket already holds an exact-signature equal function.
    /// - `Rebind` if `data[name]` holds a non-function.
    pub fn try_register_function(
        &self,
        name: &str,
        fn_ref: &'a KFunction<'a>,
        obj: &'a KObject<'a>,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply(name, obj, Some(fn_ref))
    }

    /// Register `name` → `kt` in the dedicated type-binding map. Errors `Rebind`
    /// if `types[name]` already exists; returns `Ok(Conflict)` on borrow
    /// contention (caller queues — same shape as [`Bindings::try_bind_value`]
    /// and [`Bindings::try_register_function`]). Best-effort placeholder clear
    /// on success.
    ///
    /// Unused at land time — wired into `Scope::register_type` in stage 1.4.
    pub fn try_register_type(
        &self,
        name: &str,
        kt: &'a KType,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply_type(name, kt)
    }

    /// Install a dispatch-time placeholder for `name` -> producer slot `idx`.
    ///
    /// Lenient when `data[name]` already holds a `KObject::KFunction`: silent no-op.
    /// Forward references resolve through the existing function value; a new FN overload
    /// joins the per-signature bucket on finalize without consumers needing to park.
    ///
    /// Errors `Rebind` if `data[name]` holds a non-function or if `placeholders[name]`
    /// already maps to a *different* `NodeId`. Idempotent if re-entered with the same
    /// `NodeId`.
    ///
    /// Unlike [`Bindings::try_bind_value`] and [`Bindings::try_register_function`], this
    /// method panics on borrow conflict rather than returning `Conflict` — placeholder
    /// installs happen at dispatch-time outside the re-entrant-bind hot path, so a conflict
    /// here indicates a programming error, not a queueable retry.
    pub fn try_install_placeholder(&self, name: String, idx: NodeId) -> Result<(), KError> {
        if let Some(existing) = self.data.borrow().get(&name) {
            if matches!(existing, KObject::KFunction(_, _)) {
                return Ok(());
            }
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        let mut ph = self.placeholders.borrow_mut();
        if let Some(existing) = ph.get(&name).copied() {
            if existing == idx {
                return Ok(());
            }
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        ph.insert(name, idx);
        Ok(())
    }

    /// Replay another `Bindings`'s `data` through `try_apply` on self. The single entry
    /// point for ascription's bulk-install — snapshots the source `data` into a `Vec` and
    /// releases `src`'s `Ref` before the replay so re-entrant ascription cannot deadlock.
    /// The shared helper re-mirrors `KFunction` entries into `functions` exactly once, so
    /// the caller does not need to walk `src.functions` separately (that's the point of
    /// routing through `try_apply`).
    ///
    /// The replay is order-independent: the dispatch bucket is order-insensitive once
    /// dedupe is applied. Panics on `Conflict` — a fresh `Bindings` should never hit a
    /// borrow conflict against itself, and a conflict here is a programming error.
    pub fn try_bulk_install_from(&self, src: &Bindings<'a>) -> Result<(), KError> {
        let snapshot: Vec<(String, &'a KObject<'a>)> = src
            .data()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        for (name, obj) in snapshot {
            let fn_part = match obj {
                KObject::KFunction(f, _) => Some(*f),
                _ => None,
            };
            match self.try_apply(&name, obj, fn_part)? {
                ApplyOutcome::Applied => {}
                ApplyOutcome::Conflict => {
                    unreachable!(
                        "try_bulk_install_from on a fresh Bindings should not hit borrow conflict",
                    );
                }
            }
        }
        Ok(())
    }

    /// The shared write path for type bindings. Borrows `types` only — no
    /// `data`/`functions` involvement, since this primitive writes a single
    /// map. `try_register_nominal` (stage 1.3) layers the dual-write on top
    /// of this helper's contract.
    ///
    /// `Conflict` is reserved for borrow contention; `Err(Rebind)` is the
    /// semantic-rejection path. On success, also clears any matching
    /// placeholder (best-effort, mirrors `try_apply`'s tail).
    fn try_apply_type(
        &self,
        name: &str,
        kt: &'a KType,
    ) -> Result<ApplyOutcome, KError> {
        let mut types = match self.types.try_borrow_mut() {
            Ok(t) => t,
            Err(_) => return Ok(ApplyOutcome::Conflict),
        };
        if types.contains_key(name) {
            return Err(KError::new(KErrorKind::Rebind { name: name.to_string() }));
        }
        types.insert(name.to_string(), kt);
        drop(types);
        if let Ok(mut ph) = self.placeholders.try_borrow_mut() {
            ph.remove(name);
        }
        Ok(ApplyOutcome::Applied)
    }

    /// The shared write path. Borrows `functions` first (only when `fn_part.is_some()`),
    /// then `data` — preserves the non-fn shortcut so `register_type`, LET-body, and
    /// param-binding flows that run under a live outer `functions` borrow stay
    /// deadlock-free. `Conflict` is reserved for borrow contention; semantic errors come
    /// through `Err(KError)`.
    ///
    /// Unified dedupe: when `fn_part.is_some()` and a same-name `data` entry exists, walk
    /// the bucket — `ptr::eq` is silent-success short-circuit (preserves intentional
    /// aliases like `LET g = (f)`), `signatures_exact_equal` is `DuplicateOverload`. Both
    /// `FN`-decl and `LET`-binds-`FN` paths see both rules. This closes the latent gap
    /// where `try_apply_value`'s pointer-only dedupe could silently double the bucket on a
    /// structurally identical but pointer-distinct re-bind.
    fn try_apply(
        &self,
        name: &str,
        obj: &'a KObject<'a>,
        fn_part: Option<&'a KFunction<'a>>,
    ) -> Result<ApplyOutcome, KError> {
        // Borrow `functions` first only when there is a function-side mirror to write —
        // skipping otherwise preserves the non-fn shortcut documented above.
        let mut functions_handle = if fn_part.is_some() {
            match self.functions.try_borrow_mut() {
                Ok(g) => Some(g),
                Err(_) => return Ok(ApplyOutcome::Conflict),
            }
        } else {
            None
        };
        let mut data = match self.data.try_borrow_mut() {
            Ok(d) => d,
            Err(_) => return Ok(ApplyOutcome::Conflict),
        };
        // Semantic rejection on existing `data[name]`. The `fn_part.is_some()` and entry-
        // already-`KFunction` case falls through to bucket dedupe (overload-add path).
        if let Some(existing) = data.get(name) {
            match fn_part {
                None => return Err(KError::new(KErrorKind::Rebind { name: name.to_string() })),
                Some(_) => {
                    if !matches!(existing, KObject::KFunction(_, _)) {
                        return Err(KError::new(KErrorKind::Rebind { name: name.to_string() }));
                    }
                }
            }
        }
        if let (Some(f_ref), Some(functions)) = (fn_part, functions_handle.as_mut()) {
            let key = f_ref.signature.untyped_key();
            let bucket = functions.entry(key).or_default();
            let mut already_present = false;
            for existing in bucket.iter() {
                if std::ptr::eq(*existing, f_ref) {
                    already_present = true;
                    break;
                }
                if signatures_exact_equal(&existing.signature, &f_ref.signature) {
                    return Err(KError::new(KErrorKind::DuplicateOverload {
                        name: name.to_string(),
                        signature: existing.summarize(),
                    }));
                }
            }
            if !already_present {
                bucket.push(f_ref);
            }
        }
        data.insert(name.to_string(), obj);
        drop(data);
        drop(functions_handle);
        // Best-effort placeholder clear — `try_borrow_mut().ok()` tolerates a caller
        // holding a placeholder borrow up the stack. Promoting this to `borrow_mut()`
        // would panic for a previously-tolerated case.
        if let Ok(mut ph) = self.placeholders.try_borrow_mut() {
            ph.remove(name);
        }
        Ok(ApplyOutcome::Applied)
    }
}

impl<'a> Default for Bindings<'a> {
    fn default() -> Self {
        Self::new()
    }
}

/// `Conflict` is reserved for borrow contention; semantic errors come through `Err(KError)`.
/// `pub` so [`super::scope`]'s match arms compile; not re-exported beyond `core::`.
pub enum ApplyOutcome {
    Applied,
    Conflict,
}

/// Structural equality on shape + per-Argument `KType` + return type. Independent of
/// `Argument::name` — two overloads with matching shape and types collide for dispatch
/// regardless of parameter naming.
fn signatures_exact_equal(
    a: &crate::runtime::model::types::ExpressionSignature,
    b: &crate::runtime::model::types::ExpressionSignature,
) -> bool {
    use crate::runtime::model::types::SignatureElement;
    if a.return_type != b.return_type {
        return false;
    }
    if a.elements.len() != b.elements.len() {
        return false;
    }
    a.elements.iter().zip(b.elements.iter()).all(|(x, y)| match (x, y) {
        (SignatureElement::Keyword(s), SignatureElement::Keyword(t)) => s == t,
        (SignatureElement::Argument(ax), SignatureElement::Argument(ay)) => ax.ktype == ay.ktype,
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    //! Unit coverage for the stage-1.2 `types` map and its `try_register_type` write
    //! primitive. The primitive has no consumer yet (stage 1.4 wires
    //! `Scope::register_type`), so these tests directly exercise `Bindings` against a
    //! `RuntimeArena`-allocated `&KType`.

    use super::*;
    use crate::runtime::machine::core::arena::RuntimeArena;
    use crate::runtime::model::types::KType;

    #[test]
    fn try_register_type_inserts_into_types_map() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let kt: &KType = arena.alloc_ktype(KType::Number);
        let outcome = bindings
            .try_register_type("Foo", kt)
            .expect("try_register_type should succeed on fresh bindings");
        assert!(matches!(outcome, ApplyOutcome::Applied));
        // Type-side storage is the only home for this binding — `data` stays empty.
        let stored = *bindings.types().get("Foo").expect("Foo should be in types map");
        assert!(std::ptr::eq(stored, kt));
        assert!(bindings.data().get("Foo").is_none());
    }

    #[test]
    fn try_register_type_rejects_collision_with_rebind() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let kt1: &KType = arena.alloc_ktype(KType::Number);
        let kt2: &KType = arena.alloc_ktype(KType::Str);
        bindings.try_register_type("Foo", kt1).expect("first register should succeed");
        let err = match bindings.try_register_type("Foo", kt2) {
            Err(e) => e,
            Ok(_) => panic!("second register on same name should error, not succeed"),
        };
        assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "Foo"));
        // First binding remains intact — the collision must not overwrite.
        let stored = *bindings.types().get("Foo").expect("Foo should still be present");
        assert!(std::ptr::eq(stored, kt1));
    }

    #[test]
    fn try_register_type_yields_conflict_on_live_types_borrow() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let kt: &KType = arena.alloc_ktype(KType::Number);
        let _r = bindings.types();
        let outcome = bindings
            .try_register_type("Foo", kt)
            .expect("conflict path returns Ok(Conflict), not Err");
        assert!(matches!(outcome, ApplyOutcome::Conflict));
        // Live read borrow blocked the write; nothing was inserted.
        assert!(_r.get("Foo").is_none());
    }

    #[test]
    fn try_register_type_clears_matching_placeholder() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let kt: &KType = arena.alloc_ktype(KType::Number);
        bindings
            .try_install_placeholder("Bar".to_string(), NodeId(7))
            .expect("placeholder install should succeed on fresh bindings");
        assert!(bindings.placeholders().contains_key("Bar"));
        bindings
            .try_register_type("Bar", kt)
            .expect("type register should succeed and clear placeholder");
        assert!(!bindings.placeholders().contains_key("Bar"));
    }

    #[test]
    fn try_register_type_does_not_touch_data_or_functions() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let kt: &KType = arena.alloc_ktype(KType::Number);
        bindings.try_register_type("Foo", kt).expect("register should succeed");
        assert!(bindings.data().is_empty());
        assert!(bindings.functions().is_empty());
    }
}
