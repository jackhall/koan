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

use crate::ast::{TypeExpr, TypeParams};
use crate::runtime::machine::NodeId;
use crate::runtime::machine::core::{Resolution, Scope};

use super::ktype::KType;

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
pub struct Elaborator<'s, 'a> {
    pub scope: &'s Scope<'a>,
    pub threaded: HashSet<String>,
    pub fired_self_ref_for: HashSet<String>,
    pub self_id: Option<NodeId>,
}

impl<'s, 'a> Elaborator<'s, 'a> {
    pub fn new(scope: &'s Scope<'a>) -> Self {
        Self {
            scope,
            threaded: HashSet::new(),
            fired_self_ref_for: HashSet::new(),
            self_id: None,
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
            "List<...> expects exactly 1 type parameter, got {}",
            items.len()
        )),
        (name, TypeParams::List(items)) if name == "Dict" && items.len() == 2 => {
            let k = elaborate_type_expr(el, &items[0]);
            let v = elaborate_type_expr(el, &items[1]);
            merge_two_into_dict(k, v)
        }
        (name, TypeParams::List(items)) if name == "Dict" => ElabResult::Unbound(format!(
            "Dict<...> expects exactly 2 type parameters, got {}",
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
        (name, TypeParams::List(_)) => ElabResult::Unbound(format!(
            "type `{name}` does not take type parameters"
        )),
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
