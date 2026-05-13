//! The lexical binding façade: three co-mutating `RefCell` maps (`data`, `functions`,
//! `placeholders`) plus the shared validated write path [`Bindings::try_apply`] that keeps
//! the dual-map invariant — every `data[name]` entry wrapping a `KFunction` lives in
//! `functions[signature.untyped_key()]` — in one place. Lives in its own module so the
//! façade's surface (write methods + read handles) stays focused; [`Scope`] embeds it by
//! value so all interior borrows arbitrate against one another.
//!
//! `ApplyOutcome` is `pub` so [`scope`](super::scope)'s match arms on the
//! [`Bindings::try_bind_value`] / [`Bindings::try_register_function`] results compile, but
//! it isn't re-exported beyond `core::` — the `Conflict` variant is an internal queueing
//! signal, not part of any user-visible API.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;

use crate::runtime::machine::kfunction::{KFunction, NodeId};
use crate::runtime::model::types::UntypedKey;
use crate::runtime::model::values::KObject;

use super::kerror::{KError, KErrorKind};

/// Façade owning the three co-mutating `RefCell` maps that back every lexical binding:
/// `data` (name → value), `functions` (untyped-signature bucket → overloads), and
/// `placeholders` (name → producer NodeId for dispatch-time forward references).
///
/// The shared private [`Bindings::try_apply`] enforces the dual-map invariant —
/// every `data[name]` entry wrapping a `KFunction` lives in
/// `functions[signature.untyped_key()]` — in one place, and unifies dedupe (`ptr::eq`
/// fast-path then `signatures_exact_equal`) across the LET-binds-FN and `FN`-decl paths.
///
/// Lifetime `'a` matches the arena lifetime of the stored references; the façade itself
/// is embedded by value on [`super::scope::Scope`] so all interior borrows arbitrate
/// against one another.
pub struct Bindings<'a> {
    data: RefCell<HashMap<String, &'a KObject<'a>>>,
    functions: RefCell<HashMap<UntypedKey, Vec<&'a KFunction<'a>>>>,
    placeholders: RefCell<HashMap<String, NodeId>>,
}

impl<'a> Bindings<'a> {
    pub fn new() -> Self {
        Self {
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
