//! `VAL <name:Identifier> : <ty:TypeExprRef>` — SIG-body-only declarator for value
//! slots whose declared type is recorded explicitly. See
//! [design/module-system.md](../../../design/module-system.md)'s "Structures and
//! signatures" section for the surface design.
//!
//! `VAL` exists exclusively inside SIG bodies — outside a SIG body its body returns a
//! structured `ShapeError`. The gate walks outward through `Anonymous` frames and pivots
//! on the first non-`Anonymous` [`ScopeKind`]: `Sig` admits VAL, `Module` rejects.
//! `sig_def::body` stamps `ScopeKind::Sig` on its decl_scope via `Scope::child_under_sig`.
//!
//! Storage: the bound name lives in `bindings.data[name] = KObject::KTypeValue(declared_kt)`.
//! The carrier overloads `KTypeValue`'s meaning inside a SIG decl_scope: at run-root or
//! a MODULE/FN scope a `data[name] = KTypeValue(_)` means "this name is bound to a type
//! value"; inside a SIG decl_scope it means "this name is a value slot whose declared
//! type is `kt`". The disambiguation is by scope context, which `ascribe::shape_check`
//! already consults. No write to `bindings.types` — VAL slots are *value* slots, not
//! abstract type members.
//!
//! Type resolution: the `ty` slot is `TypeExprRef`. Two carrier shapes arrive, plus a
//! synchronous-resolve fast path:
//!
//! - `KObject::KTypeValue(kt)` for a **leaf builtin** (`Number`, `Type`, `Str`, ...)
//!   — `resolve_for` lowered the surface `Type(_)` token via `KType::from_type_expr`
//!   (builtin-table only, no scope awareness). We sub-Dispatch a single-part
//!   `[Type(te)]` expression against `decl_scope` so a SIG-local `LET <name> = ...`
//!   shadow wins over the builtin-table fallback (`value_lookup::body_type_expr`
//!   consults `Scope::resolve_type` which walks `bindings.types`, finding the shadow
//!   first). The sub-Dispatch parks on a placeholder when the leaf has an in-flight
//!   sibling LET — the dispatcher's replay-park machinery installs a Notify edge
//!   (not Owned), so the SIG outer Combine retains exclusive cascade-free ownership
//!   of the sibling LET. See `schedule_type_resolve` for the avoid-double-ownership
//!   rationale.
//!
//! - `KObject::KTypeValue(kt)` for a **structural** type (`KFunction`, `List`,
//!   `Dict`, `UserType`, `AnyUserType`, `SignatureBound`, `ConstructorApply`,
//!   `Mu`/`RecursiveRef`) — lifted directly. The structural form is already resolved;
//!   inner builtin names don't re-bind against decl_scope shadowing (rendering then
//!   re-parsing would lose structure, and the shape-check at ascription time is
//!   name-presence only so the inner identity distinction doesn't affect ascription
//!   today; modular implicits will revisit when full type-shape checking lands).
//!
//! - `KObject::TypeNameRef(t, _)` — `from_type_expr` failed (some leaf in the
//!   `TypeExpr` isn't in the builtin table); the carrier preserves the raw shape.
//!   Bare-leaf forms (`TypeParams::None`) ride the same sub-Dispatch path as the
//!   leaf-builtin case. Parameterized / function-shaped forms (`Wrap<Number>`,
//!   `Function<(Number) -> Wrap<Number>>`) try synchronous elaboration against
//!   `decl_scope` first; on park, we collect the leaf names the elaboration
//!   references and sub-Dispatch each through `value_lookup` (which the dispatcher
//!   parks per-leaf with a Notify edge), then re-elaborate the full structural
//!   form in the Combine finish once every leaf has terminalized. This per-leaf
//!   sub-Dispatch breakup is what keeps the SIG outer Combine's exclusive
//!   cascade-free ownership of sibling LET-binder producers intact —
//!   `defer_val_via_combine` and `defer_val_structural_via_combine` document the
//!   detailed flow.

use crate::runtime::machine::core::ResolveTypeExprOutcome;
use crate::runtime::machine::model::ast::{ExpressionPart, KExpression, TypeExpr, TypeParams};
use crate::runtime::machine::{
    ArgumentBundle, BodyResult, CombineFinish, KError, KErrorKind, NodeId, Scope,
    SchedulerHandle,
};
use crate::runtime::machine::model::{KObject, KType};

use super::{arg, err, kw, register_builtin_with_pre_run, sig};

/// Sub-dispatch a single-part `[Type(te)]` expression against `decl_scope` so the
/// dispatcher's replay-park machinery handles any sibling placeholder on `te.name`
/// without VAL's Combine taking ownership of the sibling producer slot.
///
/// Why not call `elaborate_type_expr` directly? The elaborator's `Resolution::
/// Placeholder` arm returns `ElabResult::Park(vec![producer_id])`, which our caller
/// would feed into `add_combine([producer_id])`. That installs an OWNED edge from
/// the SIG-sibling LET (the placeholder's producer) to VAL's Combine — turning the
/// LET into VAL's owned child. The SIG outer Combine ALSO marks that LET as its own
/// owned child via its own `add_combine([LET_id, VAL_id])`, and Combine-finish
/// cascade-frees owned children. Two combines cascade-freeing the same producer is
/// a double-free in the slot table and reads the producer's terminal after free
/// from whichever combine finishes second.
///
/// Sub-dispatching `[Type(te)]` routes through `value_lookup::body_type_expr`. The
/// dispatcher's pickability path installs a *park* edge (Notify, not Owned) on a
/// placeholder hit via `apply_replay_park`. The park edge wakes our Combine without
/// transferring ownership, so the SIG outer Combine's cascade-free remains the sole
/// owner of LET.
fn schedule_type_resolve<'a>(
    sched: &mut dyn SchedulerHandle<'a>,
    decl_scope: &'a Scope<'a>,
    te: &TypeExpr,
) -> crate::runtime::machine::NodeId {
    let expr = KExpression {
        parts: vec![ExpressionPart::Type(te.clone())],
    };
    sched.add_dispatch(expr, decl_scope)
}

/// Classify the dispatcher's resolved carrier into one of three buckets the body
/// then routes through:
///
/// - `Leaf(te)` — a builtin leaf `KType` synthesized into a leaf `TypeExpr` so the
///   SIG-local `bindings.types` shadow has a chance to win over the builtin-table
///   fallback. Routes through the sub-Dispatch + Combine wake path.
/// - `Raw(te)` — the parser-preserved `TypeExpr` from a `TypeNameRef` carrier.
///   Same sub-Dispatch path as `Leaf` — `value_lookup::body_type_expr` handles
///   both shadow lookups and user-declared SIG-local types uniformly.
/// - `Direct(kt)` — a structural carrier the body lifts as-is, bypassing the
///   sub-Dispatch.
fn typeexpr_from_carrier<'a>(obj: &KObject<'a>) -> Result<CarrierForm, KError> {
    match obj {
        KObject::KTypeValue(kt) => match kt {
            KType::Number
            | KType::Str
            | KType::Bool
            | KType::Null
            | KType::Type
            | KType::Signature
            | KType::Any
            | KType::Identifier
            | KType::KExpression
            | KType::TypeExprRef => Ok(CarrierForm::Leaf(TypeExpr::leaf(kt.name()))),
            _ => Ok(CarrierForm::Direct(kt.clone())),
        },
        KObject::TypeNameRef(te) => Ok(CarrierForm::Raw(te.clone())),
        other => Err(KError::new(KErrorKind::TypeMismatch {
            arg: "ty".to_string(),
            expected: "TypeExprRef".to_string(),
            got: other.ktype().name(),
        })),
    }
}

enum CarrierForm {
    /// Bare-leaf carrier synthesized from `kt.name()`: re-elaborate against decl_scope
    /// so a SIG-local `LET <name> = ...` shadow wins.
    Leaf(TypeExpr),
    /// Raw parser-preserved `TypeExpr` from a `TypeNameRef` carrier: elaborate as-is.
    Raw(TypeExpr),
    /// Structural carrier (`KFunction`, `List`, `Dict`, `UserType`, `AnyUserType`,
    /// `SignatureBound`, ...) accepted directly. Inner builtin names don't re-bind
    /// against decl_scope shadowing — a known limitation documented above.
    Direct(KType),
}

pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // Gate: VAL is meaningful only inside a SIG body. Outside, the user almost
    // certainly meant `LET`; surface a focused diagnostic naming both surfaces.
    if !scope.is_in_sig_body() {
        return err(KError::new(KErrorKind::ShapeError(
            "VAL is only valid inside a SIG body — use LET for value bindings in \
             modules and run-root scope"
                .to_string(),
        )));
    }

    let name = match bundle.get("name") {
        Some(KObject::KString(s)) => s.clone(),
        Some(other) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "name".to_string(),
                expected: "Identifier".to_string(),
                got: other.ktype().name(),
            }));
        }
        None => return err(KError::new(KErrorKind::MissingArg("name".to_string()))),
    };

    // Defense-in-depth: the `name` slot is `KType::Identifier` so a Type-classified
    // token at the binder position is already rejected at dispatch shape-check time.
    // The check stays here so a future routing change (e.g. extending the name slot
    // to accept Type tokens for SIG-local type-class binders) doesn't silently
    // misroute Type-class-named slots through VAL. Abstract-type members must use
    // `LET TypeName = ...`, not `VAL`.
    if super::ascribe::is_abstract_type_name(&name) {
        return err(KError::new(KErrorKind::ShapeError(format!(
            "VAL slot name `{name}` classifies as a Type token; abstract-type members \
             must use `LET {name} = <Type>` instead of `VAL`",
        ))));
    }

    let ty_obj = match bundle.get("ty") {
        Some(o) => o,
        None => return err(KError::new(KErrorKind::MissingArg("ty".to_string()))),
    };
    let carrier = match typeexpr_from_carrier(ty_obj) {
        Ok(c) => c,
        Err(e) => return err(e),
    };

    match carrier {
        // Structural carrier: lift `kt` as-is. No scope-aware re-elaboration needed
        // (the dispatcher already structurally resolved through the builtin table;
        // inner shadowing isn't honored, see the file-top docstring).
        CarrierForm::Direct(kt) => finalize_val(scope, name, kt),
        // Bare-leaf carrier: route through a sub-Dispatch on
        // `value_lookup::body_type_expr`. The dispatcher's replay-park machinery
        // handles any sibling-placeholder hit via a Notify edge (no shared
        // ownership of the SIG-sibling producer slot — see `schedule_type_resolve`'s
        // docstring). The Combine's finish reads the resolved `KTypeValue` carrier
        // and finalizes the bind.
        CarrierForm::Leaf(te) => {
            let resolve_id = schedule_type_resolve(sched, scope, &te);
            defer_val_via_combine(scope, sched, name, te, resolve_id)
        }
        // Parser-preserved `TypeNameRef` carrier. The bare-leaf shape rides the same
        // sub-Dispatch path as `Leaf`. Parameterized / function-shaped forms
        // (`Wrap<Number>`, `Function<(Number) -> Wrap<Number>>`) attempt a
        // synchronous elaboration against `decl_scope`; on park, the leaf names the
        // elaboration needs are sub-Dispatched individually (each via the bare-leaf
        // `[Type(leaf)]` shape `value_lookup::body_type_expr` accepts), and a
        // Combine over those sub-Dispatches re-elaborates the structural form once
        // every leaf has terminalized. The per-leaf sub-Dispatches park on their
        // placeholders via the dispatcher's replay-park (Notify, not Owned), so the
        // SIG outer Combine retains exclusive cascade-free ownership of the sibling
        // LET-binders.
        CarrierForm::Raw(te) => {
            if matches!(te.params, TypeParams::None) {
                let resolve_id = schedule_type_resolve(sched, scope, &te);
                return defer_val_via_combine(scope, sched, name, te, resolve_id);
            }
            match scope.resolve_type_expr(&te) {
                ResolveTypeExprOutcome::Done(kt) => finalize_val(scope, name, kt.clone()),
                ResolveTypeExprOutcome::Park(_) => {
                    // Collect the leaf names the structural form references; sub-Dispatch
                    // each through `value_lookup` so the dispatcher's replay-park installs
                    // a Notify edge on each placeholder. When all leaf sub-Dispatches
                    // terminalize, our Combine re-elaborates the full structural form
                    // against the now-final `decl_scope`.
                    let leaves = collect_leaf_names(&te);
                    let mut dep_ids: Vec<NodeId> = Vec::with_capacity(leaves.len());
                    for leaf in &leaves {
                        let leaf_te = TypeExpr::leaf(leaf.clone());
                        dep_ids.push(schedule_type_resolve(sched, scope, &leaf_te));
                    }
                    defer_val_structural_via_combine(scope, sched, name, te, dep_ids)
                }
                ResolveTypeExprOutcome::Unbound(msg) => {
                    err(KError::new(KErrorKind::ShapeError(format!("VAL type: {msg}"))))
                }
            }
        }
    }
}

/// Walk `te` and return the set of bare-leaf names that appear inside it (the
/// structural form's "free" type-name references). Used by the parked-structural
/// path so each leaf can sub-Dispatch independently through `value_lookup`,
/// installing one Notify edge per placeholder hit.
fn collect_leaf_names(te: &TypeExpr) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    fn walk(
        te: &TypeExpr,
        out: &mut Vec<String>,
        seen: &mut std::collections::HashSet<String>,
    ) {
        match &te.params {
            TypeParams::None => {
                if seen.insert(te.name.clone()) {
                    out.push(te.name.clone());
                }
            }
            TypeParams::List(items) => {
                // The outer constructor name participates as a leaf reference itself
                // (e.g. `Wrap<Number>` needs to resolve `Wrap` against decl_scope).
                if seen.insert(te.name.clone()) {
                    out.push(te.name.clone());
                }
                for it in items {
                    walk(it, out, seen);
                }
            }
            TypeParams::Function { args, ret } => {
                // `Function<...>` is itself a builtin keyword — no need to resolve it as
                // a name. Recurse into args / ret only.
                for a in args {
                    walk(a, out, seen);
                }
                walk(ret, out, seen);
            }
        }
    }
    walk(te, &mut out, &mut seen);
    out
}

/// Finalize: bind `name` under `scope.bindings.data` carrying the resolved declared
/// `KType` as `KObject::KTypeValue(kt)`. Shared between the `Direct(kt)` synchronous
/// path (structural carriers) and the Combine-finish path (leaf / `TypeNameRef`
/// sub-Dispatched resolves).
fn finalize_val<'a>(scope: &'a Scope<'a>, name: String, declared_kt: KType) -> BodyResult<'a> {
    let arena = scope.arena;
    let allocated: &'a KObject<'a> = arena.alloc_object(KObject::KTypeValue(declared_kt));
    if let Err(e) = scope.bind_value(name, allocated) {
        return err(e);
    }
    BodyResult::Value(allocated)
}

/// Schedule a Combine over the type-resolution sub-Dispatch and finalize the bind
/// in the finish closure. The Combine has a single dep — the sub-Dispatch's NodeId
/// — and reads the resolved `KTypeValue` carrier from `results[0]`. Any earlier
/// `Unbound` from the sub-Dispatch comes through `run_combine`'s short-circuit
/// (every dep is checked for errored terminal before the closure fires) so the
/// closure body assumes a `KTypeValue` carrier on `results[0]`.
fn defer_val_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    name: String,
    te: TypeExpr,
    resolve_id: NodeId,
) -> BodyResult<'a> {
    let name_for_finish = name;
    let te_for_finish = te;
    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
        debug_assert_eq!(results.len(), 1, "VAL Combine has exactly one dep");
        let resolved = results[0];
        let kt = match resolved {
            KObject::KTypeValue(kt) => kt.clone(),
            // Defensive: the sub-Dispatch routed through `value_lookup::body_type_expr`
            // which always returns either a `KTypeValue` or an `UnboundName` error.
            // An error short-circuits the Combine via `run_combine`'s dep-error
            // propagation, so by the time the closure runs we have a value-shaped
            // result. A non-`KTypeValue` value here would be a routing bug —
            // surface as a structured error rather than panicking.
            other => {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                    "VAL type `{}` sub-dispatch resolved to a non-type value of kind `{}`",
                    te_for_finish.render(),
                    other.ktype().name(),
                ))));
            }
        };
        finalize_val(scope, name_for_finish.clone(), kt)
    });
    let combine_id = sched.add_combine(vec![resolve_id], scope, finish);
    BodyResult::DeferTo(combine_id)
}

/// Schedule a Combine over the per-leaf type-resolution sub-Dispatches and
/// re-elaborate the structural `TypeExpr` in the finish closure. Each leaf
/// sub-Dispatch parks on its placeholder via the dispatcher's replay-park
/// machinery (Notify edge), so the SIG outer Combine retains exclusive
/// cascade-free ownership of any sibling LET-binder producers. The Combine's
/// finish runs after every leaf has terminalized; the synchronous re-elaboration
/// then succeeds because every leaf's binding lives in `decl_scope.bindings.types`
/// at that point.
fn defer_val_structural_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    name: String,
    te: TypeExpr,
    dep_ids: Vec<NodeId>,
) -> BodyResult<'a> {
    let name_for_finish = name;
    let te_for_finish = te;
    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, _results| {
        // Every dep terminalized successfully (errored deps short-circuit through
        // `run_combine`'s dep-error propagation before reaching this closure). All
        // referenced leaf names now have entries in `decl_scope.bindings.types`, so
        // the elaborator's bare-leaf arm resolves them synchronously.
        match scope.resolve_type_expr(&te_for_finish) {
            ResolveTypeExprOutcome::Done(kt) => finalize_val(scope, name_for_finish.clone(), kt.clone()),
            ResolveTypeExprOutcome::Park(_) => BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                "VAL type `{}` elaboration parked again after all leaf sub-Dispatches \
                 terminalized — internal scheduling invariant violated",
                te_for_finish.render(),
            )))),
            ResolveTypeExprOutcome::Unbound(msg) => {
                BodyResult::Err(KError::new(KErrorKind::ShapeError(format!("VAL type: {msg}"))))
            }
        }
    });
    let combine_id = sched.add_combine(dep_ids, scope, finish);
    BodyResult::DeferTo(combine_id)
}

/// Dispatch-time placeholder extractor. `parts[1]` is the bound name's `Identifier(s)`
/// token. Same shape as `let_binding::pre_run`'s Identifier arm.
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    match expr.parts.get(1)? {
        ExpressionPart::Identifier(s) => Some(s.clone()),
        _ => None,
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // `VAL` returns the bound `KTypeValue` carrier so the surface form
    // `(VAL name :Type)` evaluates to a value, mirroring `LET`'s return shape. The
    // Design-B sigil consumes the `:`, so the signature has no explicit colon keyword.
    register_builtin_with_pre_run(
        scope,
        "VAL",
        sig(KType::Any, vec![
            kw("VAL"),
            arg("name", KType::Identifier),
            arg("ty", KType::TypeExprRef),
        ]),
        body,
        Some(pre_run),
    );
}

#[cfg(test)]
mod tests {
    use crate::runtime::builtins::test_support::{
        parse_one, run, run_one_err, run_root_silent,
    };
    use crate::runtime::machine::{KErrorKind, RuntimeArena};
    use crate::runtime::machine::model::{KObject, KType};

    /// Smoke test: `(VAL zero: Number)` inside a SIG body binds `zero` under the SIG's
    /// decl_scope as a `KTypeValue(KType::Number)` carrier. The slot exists in
    /// `bindings.data` so `ascribe::shape_check` will require it of an ascribed module.
    #[test]
    fn val_inside_sig_binds_typeexpr_carrier() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG OrderedSig = ((VAL zero :Number))");
        let s = match scope.bindings().data().get("OrderedSig") {
            Some(KObject::KSignature(s)) => *s,
            _ => panic!("OrderedSig must bind a KSignature"),
        };
        let zero = s.decl_scope().bindings().expect_value("zero");
        match zero {
            KObject::KTypeValue(kt) => assert_eq!(*kt, KType::Number),
            other => panic!("expected KTypeValue(Number), got {:?}", other.ktype()),
        }
    }

    /// SIG-local shadowing: `LET Type = Number` inside the SIG body shadows the builtin
    /// `Type`. A subsequent `(VAL zero: Type)` re-elaborates against the SIG decl_scope's
    /// types map and binds `zero` with `KType::Number` (the shadow), not `KType::Type`
    /// (the meta-type). Pins the parking path — sibling statement order isn't
    /// guaranteed, so VAL parks on LET's placeholder and resumes via Combine.
    #[test]
    fn val_resolves_sig_local_type_shadow() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG WithZero = ((LET Type = Number) (VAL zero :Type))",
        );
        let s = match scope.bindings().data().get("WithZero") {
            Some(KObject::KSignature(s)) => *s,
            _ => panic!("WithZero must bind a KSignature"),
        };
        let zero = s.decl_scope().bindings().expect_value("zero");
        match zero {
            KObject::KTypeValue(kt) => assert_eq!(
                *kt, KType::Number,
                "SIG-local `LET Type = Number` must shadow the meta-type builtin",
            ),
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// `VAL` outside a SIG body — at the run-root — surfaces a structured `ShapeError`
    /// directing the user to `LET`. Gate is the immediate-enclosing labeled scope check.
    #[test]
    fn val_outside_sig_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("VAL x :Number"));
        match &err.kind {
            KErrorKind::ShapeError(msg) => {
                assert!(
                    msg.contains("VAL is only valid inside a SIG body"),
                    "expected SIG-only diagnostic, got: {msg}",
                );
            }
            _ => panic!("expected ShapeError, got something else"),
        }
    }

    /// `VAL` inside a MODULE body — modules are not SIGs; surface the same diagnostic.
    /// The immediate enclosing labeled scope is `"MODULE ..."`, not `"SIG ..."`.
    #[test]
    fn val_inside_module_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(
            scope,
            parse_one("MODULE Foo = ((VAL x :Number))"),
        );
        match &err.kind {
            KErrorKind::ShapeError(msg) => {
                assert!(
                    msg.contains("VAL is only valid inside a SIG body"),
                    "expected SIG-only diagnostic, got: {msg}",
                );
            }
            _ => panic!("expected ShapeError, got something else"),
        }
    }

    /// `(VAL compare: Function<(Number, Number) -> Number>)` — structural type carrier.
    /// The dispatcher's eager `from_type_expr` lowering produces
    /// `KFunction { args: [Number, Number], ret: Number }`; the body accepts the result
    /// directly because the structural form has no SIG-local shadow to honor.
    #[test]
    fn val_function_typed_slot() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = ((VAL compare :(Function (Number Number) -> Number)))",
        );
        let s = match scope.bindings().data().get("OrderedSig") {
            Some(KObject::KSignature(s)) => *s,
            _ => panic!("OrderedSig must bind a KSignature"),
        };
        let compare = s.decl_scope().bindings().expect_value("compare");
        match compare {
            KObject::KTypeValue(KType::KFunction { args, ret }) => {
                assert_eq!(args.len(), 2);
                assert_eq!(args[0], KType::Number);
                assert_eq!(args[1], KType::Number);
                assert_eq!(**ret, KType::Number);
            }
            other => panic!("expected KFunction-typed slot, got {:?}", other.ktype()),
        }
    }

    /// VAL on a SIG body whose name is later required by ascription: the missing-member
    /// shape-check still fires because `shape_check` walks `bindings.data` and VAL
    /// writes there.
    #[test]
    fn val_slot_required_by_shape_check() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG WithCompare = ((VAL compare :Number))\n\
             MODULE Empty = (LET unrelated = 0)",
        );
        let err = run_one_err(scope, parse_one("Empty :| WithCompare"));
        match &err.kind {
            KErrorKind::ShapeError(msg) => {
                assert!(
                    msg.contains("compare") && msg.contains("WithCompare"),
                    "expected diagnostic naming missing `compare`, got: {msg}",
                );
            }
            _ => panic!("expected ShapeError, got something else"),
        }
    }

    /// A MODULE that supplies the VAL-declared slot via a regular `LET name = <value>`
    /// satisfies the SIG. The shape_check is name-presence only; the VAL's declared
    /// type is recorded but not yet checked against the example value's `ktype()` —
    /// that's modular implicits.
    #[test]
    fn val_slot_satisfied_by_module_let_member() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG WithCompare = ((VAL compare :Number))\n\
             MODULE IntOrd = (LET compare = 0)\n\
             LET Ord = (IntOrd :| WithCompare)",
        );
        let data = scope.bindings().data();
        assert!(matches!(data.get("Ord"), Some(KObject::KModule(_, _))));
    }

    /// SIG body mixing the abstract type declaration (`LET Type = Number`) with a VAL
    /// slot referencing it. Pins the canonical roadmap form.
    #[test]
    fn val_with_abstract_type_member_declaration() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG WithZero = ((LET Type = Number) (VAL zero :Type))",
        );
        let s = match scope.bindings().data().get("WithZero") {
            Some(KObject::KSignature(s)) => *s,
            _ => panic!("WithZero must bind a KSignature"),
        };
        // `Type` lives in the SIG's `bindings.types`; `zero` lives in `bindings.data`.
        let type_kt = s.decl_scope().bindings().expect_type("Type");
        assert_eq!(*type_kt, KType::Number);
        let zero = s.decl_scope().bindings().expect_value("zero");
        assert!(matches!(zero, KObject::KTypeValue(KType::Number)));
    }
}
