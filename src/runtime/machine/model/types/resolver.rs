//! Scheduler-aware type-name elaboration. Walks a [`TypeExpr`] against a [`Scope`],
//! threads a "currently elaborating" set so recursive type definitions short-circuit to
//! [`KType::RecursiveRef`] instead of deadlocking on their own placeholder, and returns
//! [`ElabResult::Park`] when a referenced type-binding placeholder hasn't finalized so
//! the caller can install dep edges and re-run the elaboration on wake.
//!
//! The phase-2 transitional `TypeResolver` trait (`NoopResolver`, `ScopeResolver`) is
//! deleted: type-name bindings live in [`Scope::bindings`]'s `types` map (stage-1 of
//! per-type identity), and consumers go through [`elaborate_type_expr`] when
//! scope-aware lookup is needed or [`KType::from_type_expr`] when only the builtin
//! table matters.

use std::collections::HashSet;

use crate::runtime::machine::model::ast::{TypeExpr, TypeParams};
use crate::runtime::machine::NodeId;
use crate::runtime::machine::core::{Resolution, Scope, ScopeId};

use super::ktype::{KType, UserTypeKind};

/// Outcome of one elaboration walk over a `TypeExpr`.
#[derive(Debug)]
pub enum ElabResult {
    /// Fully elaborated to a concrete `KType`. The caller's `Mu`-wrap decision rides on
    /// the elaborator's `fired_self_ref` flag.
    Done(KType),
    /// One or more referenced type-binding placeholders haven't finalized. The caller
    /// installs park edges on every producer in `producers` and re-runs the elaboration
    /// (via Combine finish) when all parking producers terminalize.
    Park(Vec<NodeId>),
    /// A bare leaf name didn't resolve anywhere in scope and isn't a builtin. Structured
    /// error for the caller to wrap in `ShapeError`.
    Unbound(String),
}

/// Per-elaboration-walk state. `threaded` carries the names of binders the walk is
/// currently elaborating (so a self-reference becomes `RecursiveRef` instead of parking on
/// its own placeholder); `fired_self_ref_for` records which threaded names actually fired
/// a back-reference so the caller knows whether to wrap the binder's `KType` in
/// `KType::Mu`. `self_id` is the binder's own dispatch slot (when known) so a trivially
/// cyclic alias (`LET T = T`) can be detected as "name resolves to placeholder which is
/// myself" and surface a structured error instead of parking forever.
///
/// `current_decl_*` carry the stage-3.2 SCC context: when the elaborator is running on
/// behalf of a named nominal binder (STRUCT / named-UNION) whose entry is in
/// `Bindings.pending_types`, the elaborator's `Resolution::Placeholder` arm records
/// dependency edges and runs DFS cycle detection from `current_decl_name`. `None` for
/// non-binder elaboration (FN signatures, LET RHS, ascription) so those sites never
/// touch `pending_types`.
pub struct Elaborator<'s, 'a> {
    pub scope: &'s Scope<'a>,
    pub threaded: HashSet<String>,
    pub fired_self_ref_for: HashSet<String>,
    pub self_id: Option<NodeId>,
    pub current_decl_name: Option<String>,
    pub current_decl_kind: Option<UserTypeKind>,
    pub current_decl_scope_id: Option<ScopeId>,
}

impl<'s, 'a> Elaborator<'s, 'a> {
    pub fn new(scope: &'s Scope<'a>) -> Self {
        Self {
            scope,
            threaded: HashSet::new(),
            fired_self_ref_for: HashSet::new(),
            self_id: None,
            current_decl_name: None,
            current_decl_kind: None,
            current_decl_scope_id: None,
        }
    }

    pub fn with_threaded<I: IntoIterator<Item = String>>(mut self, names: I) -> Self {
        self.threaded.extend(names);
        self
    }

    pub fn with_self_id(mut self, id: NodeId) -> Self {
        self.self_id = Some(id);
        self
    }

    /// Stage-3.2 SCC seed: mark this elaborator as running on behalf of a named
    /// nominal binder so the `Resolution::Placeholder` arm records dependency
    /// edges into `Bindings.pending_types` and runs cycle detection.
    /// `Bindings.insert_pending_type` is the *writer* side this hooks into;
    /// callers must install the matching `PendingTypeEntry` before launching
    /// the elaborator.
    pub fn with_current_decl(
        mut self,
        name: String,
        kind: UserTypeKind,
        scope_id: ScopeId,
    ) -> Self {
        self.current_decl_name = Some(name);
        self.current_decl_kind = Some(kind);
        self.current_decl_scope_id = Some(scope_id);
        self
    }
}

/// Walk a `TypeExpr` against the elaborator's scope. Container / function shapes recurse,
/// accumulating any `Park` producers across inner slots into a single combined park list
/// so the caller can register every dep at once. Bare-leaf names route through the
/// elaborator's threaded set first (recursive back-edge), then `Scope::resolve_type` for
/// every type-side binding (builtin type names, `LET`-bound type names, plus user-
/// declared STRUCT / UNION / MODULE / SIG names dual-written into `bindings.types` by
/// the finalize sites). `Resolution::Placeholder` is the dispatch-time forward
/// reference path; `Resolution::Value` and `Resolution::Unbound` fall through to
/// `KType::from_name` covering test fixtures that skip `default_scope`'s builtin
/// registration. A genuinely unbound leaf surfaces as `ElabResult::Unbound`.
pub fn elaborate_type_expr(
    el: &mut Elaborator<'_, '_>,
    t: &TypeExpr,
) -> ElabResult {
    match (&t.name, &t.params) {
        (name, TypeParams::None) => {
            if el.threaded.contains(name) {
                el.fired_self_ref_for.insert(name.clone());
                return ElabResult::Done(KType::RecursiveRef(name.clone()));
            }
            // Type-side first: walk `bindings.types` via `resolve_type`. Owns every
            // builtin type name post-stage-1.4 and will own stage-3 `KType::UserType`
            // entries. The `Scope::resolve` fallback that previously synthesized a
            // `KObject::KTypeValue` from this same map at lookup time is gone — the
            // `resolve_type` call here covers that path directly.
            if let Some(kt) = el.scope.resolve_type(name) {
                return ElabResult::Done(kt.clone());
            }
            match el.scope.resolve(name) {
                Resolution::Placeholder(id) => {
                    // Trivial cycle: `LET T = T` — the only producer we'd park on is
                    // ourselves. Surface as Unbound (caller maps to a structured cycle
                    // error) rather than queueing a self-park that can never wake.
                    if Some(id) == el.self_id {
                        return ElabResult::Unbound(format!("cycle in type alias `{name}`"));
                    }
                    // Stage-3.2 SCC: if this elaborator runs on behalf of a named nominal
                    // binder (`current_decl_name`), the parked-on name is itself a
                    // potential in-flight binder. Record the edge unconditionally — the
                    // parked-on name may not be in `pending_types` yet (its body hasn't
                    // dispatched) but will install itself later; DFS from each newly-added
                    // edge sees the persistent edge list and detects the closing cycle at
                    // the moment the second binder records its reciprocal edge.
                    if let Some(decl) = el.current_decl_name.clone() {
                        el.scope.bindings().record_pending_edge(&decl, name.clone());
                        if let Some(members) = detect_pending_cycle(el.scope, &decl) {
                            close_type_cycle(el.scope, &members);
                            // Cycle-close synchronously installed every member's identity
                            // into `bindings.types`; the parked-on `name` is a cycle
                            // member, so `resolve_type` now returns Some.
                            if let Some(kt) = el.scope.resolve_type(name) {
                                return ElabResult::Done(kt.clone());
                            }
                        }
                    }
                    ElabResult::Park(vec![id])
                }
                // Stage 3.1: STRUCT / UNION / MODULE / SIG finalize dual-writes the
                // nominal identity into `bindings.types`, so the `resolve_type` hit
                // above covers every user-declared type name. The value-side
                // `Resolution::Value` carriers (StructType, TaggedUnionType, KSignature)
                // are no longer consulted here; fall through to `from_name` so
                // fixture-shaped tests that skip `default_scope`'s builtin registration
                // still resolve builtin leaf names.
                Resolution::Value(_) | Resolution::Unbound => match KType::from_name(name) {
                    Some(kt) => ElabResult::Done(kt),
                    None => ElabResult::Unbound(name.clone()),
                },
            }
        }
        (name, TypeParams::List(items)) if name == "List" && items.len() == 1 => {
            match elaborate_type_expr(el, &items[0]) {
                ElabResult::Done(kt) => ElabResult::Done(KType::List(Box::new(kt))),
                other => other,
            }
        }
        (name, TypeParams::List(items)) if name == "List" => ElabResult::Unbound(format!(
            ":(List ...) expects exactly 1 type parameter, got {}",
            items.len()
        )),
        (name, TypeParams::List(items)) if name == "Dict" && items.len() == 2 => {
            let k = elaborate_type_expr(el, &items[0]);
            let v = elaborate_type_expr(el, &items[1]);
            merge_two_into_dict(k, v)
        }
        (name, TypeParams::List(items)) if name == "Dict" => ElabResult::Unbound(format!(
            ":(Dict ...) expects exactly 2 type parameters, got {}",
            items.len()
        )),
        (name, TypeParams::Function { args, ret }) if name == "Function" => {
            let mut arg_kts: Vec<KType> = Vec::with_capacity(args.len());
            let mut parks: Vec<NodeId> = Vec::new();
            let mut unbound: Option<String> = None;
            for a in args {
                match elaborate_type_expr(el, a) {
                    ElabResult::Done(kt) => arg_kts.push(kt),
                    ElabResult::Park(ps) => parks.extend(ps),
                    ElabResult::Unbound(m) if unbound.is_none() => unbound = Some(m),
                    ElabResult::Unbound(_) => {}
                }
            }
            let ret_kt = elaborate_type_expr(el, ret);
            match ret_kt {
                ElabResult::Done(rt) => {
                    if let Some(msg) = unbound {
                        ElabResult::Unbound(msg)
                    } else if !parks.is_empty() {
                        ElabResult::Park(parks)
                    } else {
                        ElabResult::Done(KType::KFunction {
                            args: arg_kts,
                            ret: Box::new(rt),
                        })
                    }
                }
                ElabResult::Park(ps) => {
                    parks.extend(ps);
                    if let Some(msg) = unbound {
                        ElabResult::Unbound(msg)
                    } else {
                        ElabResult::Park(parks)
                    }
                }
                ElabResult::Unbound(m) => ElabResult::Unbound(m),
            }
        }
        (name, TypeParams::List(items)) => {
            // Module-system stage 2: scope-aware constructor application. The outer
            // name may resolve to a `KType::UserType { kind: UserTypeKind::TypeConstructor
            // { param_names }, .. }` — a per-call-minted higher-kinded slot — in which
            // case we arity-check args against `param_names.len()`, recurse-elaborate
            // each arg, and emit a structural `KType::ConstructorApply`. Two outer-name
            // lookup paths participate (mirror of the bare-leaf arm above):
            //
            // - `Scope::resolve_type(name)` — the type-side map (LET Type-class aliases
            //   land here via `register_type`). The per-call-minted TypeConstructor
            //   identity lives here once the LET completes.
            // - `Scope::resolve` for a `Resolution::Placeholder(id)` — the LET-binding
            //   hasn't terminalized yet. Park on the producer; the Combine wake re-runs
            //   the elaboration against the now-final scope (mirror of the bare-leaf
            //   arm's placeholder path).
            //
            // Threaded-binder self-references aren't supported here (`name == self_id`'s
            // case is handled in the `TypeParams::None` arm above and emits
            // `RecursiveRef`); a self-reference of the form `Wrap<T>` is rejected as a
            // structural recursion until phase 3 introduces threaded unfold sets.
            if let Some(ctor_kt) = el.scope.resolve_type(name) {
                if let KType::UserType {
                    kind: UserTypeKind::TypeConstructor { param_names },
                    ..
                } = ctor_kt
                {
                    if items.len() != param_names.len() {
                        return ElabResult::Unbound(format!(
                            "type constructor `{name}` expects {} type parameter(s), got {}",
                            param_names.len(),
                            items.len(),
                        ));
                    }
                    let mut arg_kts: Vec<KType> = Vec::with_capacity(items.len());
                    let mut parks: Vec<NodeId> = Vec::new();
                    let mut unbound: Option<String> = None;
                    for it in items {
                        match elaborate_type_expr(el, it) {
                            ElabResult::Done(kt) => arg_kts.push(kt),
                            ElabResult::Park(ps) => parks.extend(ps),
                            ElabResult::Unbound(m) if unbound.is_none() => unbound = Some(m),
                            ElabResult::Unbound(_) => {}
                        }
                    }
                    if let Some(msg) = unbound {
                        return ElabResult::Unbound(msg);
                    }
                    if !parks.is_empty() {
                        return ElabResult::Park(parks);
                    }
                    return ElabResult::Done(KType::ConstructorApply {
                        ctor: Box::new(ctor_kt.clone()),
                        args: arg_kts,
                    });
                }
            }
            // Forward-reference path: `Wrap` may be an in-flight `LET Wrap = ...` whose
            // placeholder is registered but whose body hasn't dispatched yet. Park on
            // the producer so the Combine wake re-runs the elaboration. Self-cycle
            // (`LET Wrap = Wrap<Number>`) routes through the same Unbound path the bare
            // leaf uses.
            if let Resolution::Placeholder(id) = el.scope.resolve(name) {
                if Some(id) == el.self_id {
                    return ElabResult::Unbound(format!("cycle in type alias `{name}`"));
                }
                return ElabResult::Park(vec![id]);
            }
            ElabResult::Unbound(format!("type `{name}` does not take type parameters"))
        }
        (name, TypeParams::Function { .. }) => ElabResult::Unbound(format!(
            "only `Function` accepts a `(args) -> ret` shape; got `{name}`"
        )),
    }
}

fn merge_two_into_dict(k: ElabResult, v: ElabResult) -> ElabResult {
    match (k, v) {
        (ElabResult::Done(kt), ElabResult::Done(vt)) => {
            ElabResult::Done(KType::Dict(Box::new(kt), Box::new(vt)))
        }
        (ElabResult::Unbound(m), _) | (_, ElabResult::Unbound(m)) => ElabResult::Unbound(m),
        (ElabResult::Park(mut a), ElabResult::Park(b)) => {
            a.extend(b);
            ElabResult::Park(a)
        }
        (ElabResult::Park(p), _) | (_, ElabResult::Park(p)) => ElabResult::Park(p),
    }
}

/// DFS over `Bindings.pending_types`' adjacency lists from `start`. Returns the cycle's
/// member list (in discovery order, starting from `start`) if any path leads back to
/// `start`; `None` otherwise.
///
/// The traversal walks `edges` of each visited pending-type entry. Names that are
/// referenced via edges but not themselves in `pending_types` (a binder not yet
/// dispatched, or a non-binder placeholder) are simply leaf-terminated — their edges
/// aren't recorded yet, so no further out-edges to follow.
pub(crate) fn detect_pending_cycle(scope: &Scope<'_>, start: &str) -> Option<Vec<String>> {
    let pending = scope.bindings().pending_types();
    if !pending.contains_key(start) {
        return None;
    }
    let mut stack: Vec<(String, usize)> = vec![(start.to_string(), 0)];
    let mut on_path: HashSet<String> = HashSet::new();
    on_path.insert(start.to_string());

    while let Some((node, idx)) = stack.last().cloned() {
        let edges = pending.get(&node).map(|e| e.edges.clone()).unwrap_or_default();
        if idx >= edges.len() {
            stack.pop();
            on_path.remove(&node);
            continue;
        }
        let next = edges[idx].clone();
        stack.last_mut().unwrap().1 = idx + 1;
        if next == start {
            // Closed back to the origin: extract the cycle members from the live
            // DFS stack (path from `start` down to `node`). Stack order is already
            // root-first.
            return Some(stack.iter().map(|(n, _)| n.clone()).collect());
        }
        if on_path.contains(&next) || !pending.contains_key(&next) {
            // Already on-path (an inner cycle not involving `start`) or a leaf:
            // skip without descending. The outer cycle from `start` may still
            // exist via a different edge.
            continue;
        }
        on_path.insert(next.clone());
        stack.push((next, 0));
    }
    None
}

/// SCC cycle-close synchronous sweep. Pre-installs each member's per-declaration
/// identity (`KType::UserType { kind, scope_id, name }`) into `Bindings.types` so the
/// elaborator's `resolve_type` lookup succeeds for cross-member references on the very
/// next call. Does NOT build carriers or write `Bindings.data` — that work is left to
/// each member's own finalize path (`finalize_struct` / `finalize_union` /
/// Combine-finish), which routes through the now-idempotent `try_register_nominal`
/// arm that observes the matching types entry and writes only the carrier.
///
/// Leaving `pending_types` entries in place is deliberate: each member's finalize is
/// the one that removes its own entry, ensuring single-source bookkeeping. The
/// rebuilt elaborator inside each finalize sees `bindings.types` populated and never
/// re-enters this function (no edge recording without a placeholder hit).
fn close_type_cycle(scope: &Scope<'_>, members: &[String]) {
    // Snapshot kind + scope_id under a single `pending_types` read borrow; release
    // before calling into Scope methods that take their own borrows.
    // Stage 4: `UserTypeKind` is no longer `Copy` (the `Newtype { repr }` variant carries
    // a `Box<KType>`). `Clone` the kind out of the borrow. STRUCT / named-UNION are the
    // only carriers that participate in SCC cycle-close — neither produces a `Newtype`
    // variant here, so this clone is always a cheap variant-tag copy.
    let identities: Vec<(String, UserTypeKind, ScopeId)> = {
        let pending = scope.bindings().pending_types();
        members
            .iter()
            .map(|n| {
                let entry = pending
                    .get(n)
                    .expect("cycle member must be in pending_types when cycle-close fires");
                (n.clone(), entry.kind.clone(), entry.scope_id)
            })
            .collect()
    };
    for (name, kind, scope_id) in identities {
        let identity = KType::UserType {
            kind,
            scope_id,
            name: name.clone(),
        };
        scope.cycle_close_install_identity(name, identity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::machine::model::ast::TypeExpr;
    use crate::runtime::machine::RuntimeArena;
    use crate::runtime::builtins::test_support::run_root_silent;

    fn leaf(n: &str) -> TypeExpr {
        TypeExpr::leaf(n.into())
    }

    fn list_typeexpr(name: &str, items: Vec<TypeExpr>) -> TypeExpr {
        TypeExpr {
            name: name.into(),
            params: TypeParams::List(items),
            builtin_cache: std::cell::OnceCell::new(),
        }
    }

    /// B2: `Wrap<Number>` where `Wrap` is bound in `bindings.types` as a
    /// `KType::UserType { kind: UserTypeKind::TypeConstructor { param_names: ["Type"] }, .. }`
    /// elaborates to `KType::ConstructorApply { ctor: <that UserType>, args: [Number] }`.
    /// Pins the constructor-application arm in `elaborate_type_expr`.
    #[test]
    fn wrap_applied_elaborates_to_constructor_apply() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        // Register a TypeConstructor under `Wrap` directly (mirrors what
        // `ascribe.rs:body_opaque` mints at runtime).
        let ctor = KType::UserType {
            kind: UserTypeKind::TypeConstructor { param_names: vec!["Type".into()] },
            scope_id: ScopeId::from_raw(0, 0xC0DE),
            name: "Wrap".into(),
        };
        scope.register_type("Wrap".into(), ctor.clone());
        // Surface form: `Wrap<Number>` — a TypeExpr with name "Wrap" + List params.
        let te = list_typeexpr("Wrap", vec![leaf("Number")]);
        let mut el = Elaborator::new(scope);
        match elaborate_type_expr(&mut el, &te) {
            ElabResult::Done(kt) => match kt {
                KType::ConstructorApply { ctor: got_ctor, args } => {
                    assert_eq!(*got_ctor, ctor);
                    assert_eq!(args, vec![KType::Number]);
                }
                other => panic!("expected ConstructorApply, got {:?}", other),
            },
            other => panic!("expected Done, got {:?}", other),
        }
    }

    /// B2: two opaque ascriptions of the same SIG mint distinct `Wrap` constructors;
    /// the `ConstructorApply`s produced by elaborating against each per-call scope
    /// must therefore differ structurally. Pins the per-call generativity property
    /// extending into the applied-form layer.
    #[test]
    fn wrap_applied_distinct_per_ascription() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let scope_a = arena.alloc_scope(crate::runtime::machine::core::Scope::child_under(scope));
        let scope_b = arena.alloc_scope(crate::runtime::machine::core::Scope::child_under(scope));
        let ctor_a = KType::UserType {
            kind: UserTypeKind::TypeConstructor { param_names: vec!["Type".into()] },
            scope_id: ScopeId::from_raw(0, 0xAAAA),
            name: "Wrap".into(),
        };
        let ctor_b = KType::UserType {
            kind: UserTypeKind::TypeConstructor { param_names: vec!["Type".into()] },
            scope_id: ScopeId::from_raw(0, 0xBBBB),
            name: "Wrap".into(),
        };
        scope_a.register_type("Wrap".into(), ctor_a.clone());
        scope_b.register_type("Wrap".into(), ctor_b.clone());

        let te = list_typeexpr("Wrap", vec![leaf("Number")]);
        let mut ela = Elaborator::new(scope_a);
        let kt_a = match elaborate_type_expr(&mut ela, &te) {
            ElabResult::Done(kt) => kt,
            other => panic!("expected Done, got {:?}", other),
        };
        let mut elb = Elaborator::new(scope_b);
        let kt_b = match elaborate_type_expr(&mut elb, &te) {
            ElabResult::Done(kt) => kt,
            other => panic!("expected Done, got {:?}", other),
        };
        // Structural inequality: ctor identities differ by scope_id.
        assert_ne!(kt_a, kt_b);
    }

    /// Arity mismatch surfaces a focused error rather than building a wrong-shape
    /// `ConstructorApply`. Pins the elaborator's arity check.
    #[test]
    fn wrap_applied_arity_mismatch_unbound() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let ctor = KType::UserType {
            kind: UserTypeKind::TypeConstructor { param_names: vec!["Type".into()] },
            scope_id: ScopeId::from_raw(0, 0xC0DE),
            name: "Wrap".into(),
        };
        scope.register_type("Wrap".into(), ctor);
        // Two args against a single-param constructor: arity mismatch.
        let te = list_typeexpr("Wrap", vec![leaf("Number"), leaf("Str")]);
        let mut el = Elaborator::new(scope);
        match elaborate_type_expr(&mut el, &te) {
            ElabResult::Unbound(msg) => {
                assert!(
                    msg.contains("expects 1") && msg.contains("got 2"),
                    "expected arity message naming counts, got: {msg}",
                );
            }
            other => panic!("expected Unbound, got {:?}", other),
        }
    }

    /// Confirms that a parked-on placeholder for `name` (LET binding hasn't run yet)
    /// reports `ElabResult::Park`, not `Unbound`. Pins the forward-reference path
    /// added to the constructor-application arm so FN-defs in a SIG body whose
    /// return type is `Wrap<...>` correctly park on the in-flight `LET Wrap = ...`.
    #[test]
    fn wrap_applied_parks_on_placeholder() {
        use crate::runtime::machine::execute::Scheduler;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        // Install a placeholder for `Wrap` (NodeId 0xDEAD won't dispatch; the test
        // only inspects the elaborator's response).
        let mut sched = Scheduler::new();
        // Reserve a NodeId for the placeholder. Use `add_dispatch` on a no-op expr so
        // the scheduler has a real node id to install.
        let dummy = sched.add_dispatch(
            crate::runtime::builtins::test_support::parse_one("LET _placeholder_target = 1"),
            scope,
        );
        scope.install_placeholder("Wrap".into(), dummy).expect("placeholder install");
        let te = list_typeexpr("Wrap", vec![leaf("Number")]);
        let mut el = Elaborator::new(scope);
        match elaborate_type_expr(&mut el, &te) {
            ElabResult::Park(ids) => {
                assert!(ids.contains(&dummy), "expected parked on the Wrap placeholder, got {:?}", ids);
            }
            other => panic!("expected Park, got {:?}", other),
        }
    }

    /// `name()` round-trip — pin the diagnostic surface form `ctor<arg>`.
    #[test]
    fn constructor_apply_name_renders_surface_form() {
        let ctor = KType::UserType {
            kind: UserTypeKind::TypeConstructor { param_names: vec!["Type".into()] },
            scope_id: ScopeId::from_raw(0, 0xC0DE),
            name: "Wrap".into(),
        };
        let app = KType::ConstructorApply {
            ctor: Box::new(ctor),
            args: vec![KType::Number],
        };
        assert_eq!(app.name(), ":(Wrap Number)");
    }
}
