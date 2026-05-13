//! Pluggable type-name resolution. Consulted before the builtin `KType::from_name` table
//! so a module-local binding can shadow a builtin of the same name.
//!
//! Phase-3 addition: [`Elaborator`] and [`elaborate_type_expr`] — a scheduler-aware
//! elaborator that walks a `TypeExpr`, threads a "currently elaborating" set so recursive
//! type definitions short-circuit to `KType::RecursiveRef` instead of deadlocking on their
//! own placeholder, and returns `ElabResult::Park(producers)` when a referenced
//! type-binding placeholder hasn't finalized so the caller can install dep edges and
//! re-run the elaboration on wake.

use std::collections::HashSet;

use crate::ast::{TypeExpr, TypeParams};
use crate::runtime::machine::NodeId;
use crate::runtime::machine::core::{Resolution, Scope};
use crate::runtime::model::values::KObject;

use super::ktype::KType;

pub trait TypeResolver {
    fn resolve(&self, name: &str) -> Option<KType>;
}

pub struct NoopResolver;

impl TypeResolver for NoopResolver {
    fn resolve(&self, _name: &str) -> Option<KType> {
        None
    }
}

pub struct ScopeResolver<'s, 'a> {
    pub scope: &'s Scope<'a>,
}

impl<'s, 'a> ScopeResolver<'s, 'a> {
    pub fn new(scope: &'s Scope<'a>) -> Self {
        Self { scope }
    }
}

impl<'s, 'a> TypeResolver for ScopeResolver<'s, 'a> {
    fn resolve(&self, name: &str) -> Option<KType> {
        let bound = self.scope.lookup(name)?;
        match bound {
            // Bindings now store the elaborated `KType` directly; clone the stored value
            // rather than re-elaborating a surface form at every lookup.
            KObject::KTypeValue(kt) => Some(kt.clone()),
            // SIG names lower to `SignatureBound` so a FN parameter typed `E: OrderedSig`
            // gets a per-sig admissibility slot rather than the catch-all `KType::Module`.
            // `sig_id` is the declaring `Signature`'s stable address; the dispatcher
            // checks it against the candidate module's `compatible_sigs` set.
            KObject::KSignature(s) => Some(KType::SignatureBound {
                sig_id: s.sig_id(),
                sig_path: s.path.clone(),
            }),
            _ => None,
        }
    }
}

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
/// elaborator's threaded set first (recursive back-edge), then `Scope::resolve`
/// (`Value(KTypeValue) | Value(KSignature) | Placeholder | Unbound`), then
/// `KType::from_name` for the builtin table. A genuinely unbound leaf surfaces as
/// `ElabResult::Unbound`.
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
            match el.scope.resolve(name) {
                Resolution::Value(obj) => match obj {
                    KObject::KTypeValue(kt) => ElabResult::Done(kt.clone()),
                    KObject::KSignature(s) => ElabResult::Done(KType::SignatureBound {
                        sig_id: s.sig_id(),
                        sig_path: s.path.clone(),
                    }),
                    // A user-bound STRUCT-name resolves to a `KObject::StructType`; a
                    // user-bound UNION-name resolves to a `KObject::TaggedUnionType`. Their
                    // dispatch identity is the singleton `KType::Struct` / `KType::Type`
                    // tag until per-declaration type identity ships; report that here so
                    // a field type like `b: TreeB` lands as a usable `KType` rather than
                    // an `Unbound` error.
                    KObject::StructType { .. } => ElabResult::Done(KType::Struct),
                    KObject::TaggedUnionType(_) => ElabResult::Done(KType::Type),
                    _ => match KType::from_name(name) {
                        Some(kt) => ElabResult::Done(kt),
                        None => ElabResult::Unbound(name.clone()),
                    },
                },
                Resolution::Placeholder(id) => {
                    // Trivial cycle: `LET T = T` — the only producer we'd park on is
                    // ourselves. Surface as Unbound (caller maps to a structured cycle
                    // error) rather than queueing a self-park that can never wake.
                    if Some(id) == el.self_id {
                        return ElabResult::Unbound(format!("cycle in type alias `{name}`"));
                    }
                    ElabResult::Park(vec![id])
                }
                Resolution::Unbound => match KType::from_name(name) {
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
