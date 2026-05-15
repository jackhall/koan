//! Type-constructor builtins — `LIST_OF`, `DICT_OF`, `FUNCTION_OF`, `MODULE_TYPE_OF`. These
//! are ordinary scheduled `KFunction`s whose inputs are `TypeExprRef`-typed slots (resolved
//! to `KObject::KTypeValue(kt)`) and whose outputs are also `KObject::KTypeValue(kt)`
//! carrying the elaborated `KType` directly. Dispatching them through the same `Dispatch`
//! / `Bind` machinery values use means a parameterized type can be assembled by
//! sub-expression evaluation: `(LIST_OF (MODULE_TYPE_OF M Type))` wakes the outer slot
//! only after the inner sub-dispatch resolves to a concrete `KType` value.
//!
//! **Why builtins rather than a parallel registration table.** The design in
//! [design/module-system.md](../../../design/module-system.md)
//! reduces type-expression evaluation to ordinary dispatch: the same scope-lookup chain,
//! the same `Bind`-waits-for-subs refinement, the same `lift_kobject` rules. No new node
//! kind, no `KType::TypeVar`, no second registration table — a `TypeExprRef`-typed binding
//! lives in `Scope::data` like any other value.
//!
//! The output of these builtins is the elaborated `KType` directly — no `TypeExpr`
//! intermediate. Consumers reach the `KType` through `KObject::as_ktype()` /
//! `extract_ktype()` and operate on the structural shape rather than the surface form.

use crate::runtime::model::{Argument, ExpressionSignature, KObject, KType, SignatureElement};
use crate::runtime::model::types::UserTypeKind;
use crate::runtime::model::types::{elaborate_type_expr, ElabResult, Elaborator, ReturnType};
use crate::runtime::machine::{ArgumentBundle, BodyResult, CombineFinish, KError, KErrorKind, Scope, SchedulerHandle};
use crate::runtime::model::values::{resolve_module, resolve_signature};

use super::ascribe::{abstract_type_names_of, is_abstract_type_name};
use super::{err, register_builtin};

/// Pull a `KObject::KTypeValue`'s inner `KType` out of an arg slot. The slot is declared
/// `KType::TypeExprRef`, so by `Argument::matches` shape-time it must be either an
/// `ExpressionPart::Type(_)` (lowered into `KTypeValue` by `resolve_for`) or a
/// `Future(KObject::KTypeValue(_))` lifted from a previous sub-dispatch. Anything else
/// reaching here is a `TypeMismatch` from the dispatcher's perspective.
fn read_ktype<'a>(bundle: &ArgumentBundle<'a>, name: &str) -> Result<KType, KError> {
    let Some(obj) = bundle.get(name) else {
        return Err(KError::new(KErrorKind::MissingArg(name.to_string())));
    };
    if let Some(kt) = obj.as_ktype() {
        return Ok(kt.clone());
    }
    Err(KError::new(KErrorKind::TypeMismatch {
        arg: name.to_string(),
        expected: "TypeExprRef".to_string(),
        got: obj.ktype().name(),
    }))
}

/// `LIST_OF <elem:TypeExprRef>` → `TypeExprRef` carrying `List<elem>`.
pub fn body_list_of<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let elem = match read_ktype(&bundle, "elem") {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    BodyResult::Value(
        scope
            .arena
            .alloc_object(KObject::KTypeValue(KType::List(Box::new(elem)))),
    )
}

/// `DICT_OF <key:TypeExprRef> <value:TypeExprRef>` → `TypeExprRef` carrying
/// `Dict<key, value>`.
pub fn body_dict_of<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let key = match read_ktype(&bundle, "key") {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    let value = match read_ktype(&bundle, "value") {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    BodyResult::Value(
        scope
            .arena
            .alloc_object(KObject::KTypeValue(KType::Dict(Box::new(key), Box::new(value)))),
    )
}

/// `FUNCTION_OF <args:KExpression> -> <ret:TypeExprRef>` → `TypeExprRef` carrying
/// `Function<(args) -> ret>`. The `args` slot is captured raw as a `KExpression` whose
/// parts are bare `Type(_)` tokens; we re-extract and elaborate each into a `KType`.
/// Parameterized inner args (`List<Number>`) come through as `Future(KTypeValue(kt))` from
/// a prior sub-dispatch; leaf `Type(t)` tokens go through the resolver-free
/// [`KType::from_type_expr`] (builtin-table only) to handle nested-parameter shapes.
pub fn body_function_of<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    use crate::ast::ExpressionPart;
    let args_expr = match bundle.get("args") {
        Some(obj) => match obj.as_kexpression() {
            Some(e) => e.clone(),
            None => {
                return err(KError::new(KErrorKind::TypeMismatch {
                    arg: "args".to_string(),
                    expected: "KExpression".to_string(),
                    got: obj.ktype().name(),
                }));
            }
        },
        None => return err(KError::new(KErrorKind::MissingArg("args".to_string()))),
    };
    let ret = match read_ktype(&bundle, "ret") {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    let mut args: Vec<KType> = Vec::with_capacity(args_expr.parts.len());
    for part in &args_expr.parts {
        match part {
            ExpressionPart::Type(t) => match KType::from_type_expr(t) {
                Ok(kt) => args.push(kt),
                Err(msg) => {
                    return err(KError::new(KErrorKind::ShapeError(format!(
                        "FUNCTION_OF args: {msg}"
                    ))));
                }
            },
            ExpressionPart::Future(KObject::KTypeValue(kt)) => args.push(kt.clone()),
            other => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "FUNCTION_OF args must be type names, got `{}`",
                    other.summarize()
                ))));
            }
        }
    }
    BodyResult::Value(
        scope.arena.alloc_object(KObject::KTypeValue(KType::KFunction {
            args,
            ret: Box::new(ret),
        })),
    )
}

/// `MODULE_TYPE_OF <m:Module> <name>` → `TypeExprRef` carrying the abstract type bound
/// under `name` in `m`'s `type_members` table. Surface analogue of `M.Type`, but reachable
/// as a scheduled call so a functor body can synthesize it from a parameter module value.
/// The `m` slot is strictly `Module`; bare Type-token operands (`MODULE_TYPE_OF Foo Type`)
/// ride the auto-wrap rails — they sub-dispatch through `value_lookup` and arrive here
/// as a `Future(KModule)`. The shared [`crate::runtime::model::values::resolve_module`] helper
/// covers both the direct `KModule` path and the `(KModule, frame)` lifted form.
pub fn body_module_type_of<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let m = match bundle.get("m") {
        Some(obj) => match resolve_module(obj, "m") {
            Ok(m) => m,
            Err(e) => return err(e),
        },
        None => return err(KError::new(KErrorKind::MissingArg("m".to_string()))),
    };
    // The `name` slot accepts a Type token (e.g. `Type`, `Elt`) — abstract type names
    // classify as Type per the token-classification rules, not Identifier. The lookup uses
    // the bare leaf name from the resolved `KType`.
    let name_kt = match read_ktype(&bundle, "name") {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    let name = name_kt.name();
    // Pull the abstract type's concrete `KType` (post-3.1: `KType::UserType { kind:
    // Module, .. }` minted by opaque ascription) out of the `type_members` table directly
    // so the consumer downstream sees the identity-bearing variant rather than a
    // re-elaborated leaf.
    let kt = match m.type_members.borrow().get(&name).cloned() {
        Some(kt) => kt,
        None => {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "module `{}` has no abstract type member `{}`",
                m.path, name
            ))));
        }
    };
    BodyResult::Value(scope.arena.alloc_object(KObject::KTypeValue(kt)))
}

/// `TYPE_CONSTRUCTOR <param:TypeExprRef>` → `TypeExprRef` carrying a *template*
/// `KType::UserType { kind: UserTypeKind::TypeConstructor { param_names: vec![<param>] }, .. }`
/// with `scope_id: 0` and a placeholder `name` (`"_typeconstructor"`). The returned value
/// is a declaration template — `ascribe.rs:body_opaque` re-mints a fresh per-call
/// `scope_id` and the binding's slot name when the surrounding SIG is opaquely ascribed,
/// mirroring how `kind: Module` abstract-type slots get minted today. Stage 2 ships
/// arity-1 only; the `param_names` slot carries exactly one entry.
///
/// The `param` slot is read through the shared `read_ktype` helper — the dispatcher
/// has already resolved either a bare `Type` token or a parameterized leaf into a
/// `KTypeValue(_)`; we surface its `name()` as the constructor's parameter symbol.
pub fn body_type_constructor<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let param_kt = match read_ktype(&bundle, "param") {
        Ok(t) => t,
        Err(e) => return err(e),
    };
    // The parameter symbol is the bare Type-token name (`T`, `Elt`, ...). Structural
    // or parameterized shapes are rejected — `(TYPE_CONSTRUCTOR List<Number>)` would
    // be meaningless as a quantifier symbol.
    let param = param_kt.name();
    BodyResult::Value(
        scope.arena.alloc_object(KObject::KTypeValue(KType::UserType {
            kind: UserTypeKind::TypeConstructor { param_names: vec![param] },
            scope_id: 0,
            name: "_typeconstructor".into(),
        })),
    )
}

/// `SIG_WITH <sig:Signature> <bindings:KExpression>` → `TypeExprRef` carrying
/// `KType::SignatureBound { sig_id, sig_path, pinned_slots }`. The `bindings` slot is a
/// `KExpression` whose parts are themselves `Expression(...)` groups, one per inner
/// `(slot_name: value)` triple. Each inner expression must match
/// `[Type(slot_name), Keyword(":"), <value>]` — bare Type-token slot names only
/// (`Type`, `Elt`); lowercase identifiers are rejected because abstract-type slots
/// classify as Type per [`is_abstract_type_name`].
///
/// Value-part shapes:
/// - `Type(t)` — bare type name or `List<Number>`-style; elaborated via
///   [`elaborate_type_expr`] against the current scope.
/// - `Future(KTypeValue(_))` — already-resolved type-value spliced from a prior
///   sub-Dispatch (re-walk after Combine wake).
/// - `Expression(...)` — parens-wrapped sub-expression that must be sub-dispatched;
///   the body schedules it through the scheduler and parks on the result via
///   `Combine` finish.
///
/// Slot-name validation: each `slot_name` must appear in the SIG's `decl_scope` as
/// an abstract-type slot (resolved via [`abstract_type_names_of`]). Unknown slot
/// names error out as `"<SigPath> has no abstract type slot `<name>`"`.
pub fn body_sig_with<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    use crate::ast::ExpressionPart;

    let s = match bundle.get("sig") {
        Some(obj) => match resolve_signature(obj, "sig") {
            Ok(s) => s,
            Err(e) => return err(e),
        },
        None => return err(KError::new(KErrorKind::MissingArg("sig".to_string()))),
    };
    let bindings_expr = match bundle.get("bindings") {
        Some(obj) => match obj.as_kexpression() {
            Some(e) => e.clone(),
            None => {
                return err(KError::new(KErrorKind::TypeMismatch {
                    arg: "bindings".to_string(),
                    expected: "KExpression".to_string(),
                    got: obj.ktype().name(),
                }));
            }
        },
        None => return err(KError::new(KErrorKind::MissingArg("bindings".to_string()))),
    };

    // Pre-walk: the bindings_expr's shape comes from the parser's peel-redundant
    // pass. `((Type: Number))` collapses to a single Expression with parts
    // `[Type, Keyword(:), Type]`; `((Type: Number) (Elt: IntOrd))` stays as two
    // top-level Expression parts each wrapping a triple. Detect both shapes here:
    // - 3 parts shaped `[Type/_, Keyword(:), _]` => the whole bindings IS one
    //   triple (single-slot case after peeling).
    // - Every part is `Expression(...)` => each is its own triple (multi-slot case).
    // Anything else is a user error with a focused message.
    fn parse_triple<'a>(
        parts: &[ExpressionPart<'a>],
        out: &mut Vec<(String, ExpressionPart<'a>, usize)>,
        idx: usize,
    ) -> Result<(), KError> {
        if parts.len() != 3 {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "SIG_WITH binding must be a `(Name: Type)` triple (3 parts), got {} parts",
                parts.len(),
            ))));
        }
        let slot_name = match &parts[0] {
            ExpressionPart::Type(t) if matches!(t.params, crate::ast::TypeParams::None) => {
                t.name.clone()
            }
            ExpressionPart::Type(t) => {
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "SIG_WITH binding name must be a bare Type token (e.g. `Type`, `Elt`), got `{}`",
                    t.render(),
                ))));
            }
            ExpressionPart::Identifier(name) => {
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "SIG_WITH binding name `{name}` must be a Type token (Capital-first, with \
                     a lowercase letter) — abstract-type slots classify as Type, not Identifier",
                ))));
            }
            other => {
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "SIG_WITH binding name must be a Type token, got `{}`",
                    other.summarize(),
                ))));
            }
        };
        match &parts[1] {
            ExpressionPart::Keyword(k) if k == ":" => {}
            other => {
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "SIG_WITH binding separator must be `:`, got `{}`",
                    other.summarize(),
                ))));
            }
        }
        out.push((slot_name, parts[2].clone(), idx));
        Ok(())
    }

    let mut triples: Vec<(String, ExpressionPart<'a>, usize)> = Vec::new();
    let parts = &bindings_expr.parts;
    let is_single_triple = parts.len() == 3
        && matches!(parts[1], ExpressionPart::Keyword(ref k) if k == ":");
    let all_expression_parts = !parts.is_empty()
        && parts.iter().all(|p| matches!(p, ExpressionPart::Expression(_)));
    if is_single_triple {
        if let Err(e) = parse_triple(parts, &mut triples, 0) {
            return err(e);
        }
    } else if all_expression_parts {
        for (idx, part) in parts.iter().enumerate() {
            let inner = match part {
                ExpressionPart::Expression(boxed) => boxed.as_ref(),
                _ => unreachable!("all_expression_parts gates this arm"),
            };
            if let Err(e) = parse_triple(&inner.parts, &mut triples, idx) {
                return err(e);
            }
        }
    } else {
        let summary: Vec<String> = parts.iter().map(|p| p.summarize()).collect();
        return err(KError::new(KErrorKind::ShapeError(format!(
            "SIG_WITH bindings must be a list of parens-wrapped `(Name: Type)` triples, \
             got `[{}]`",
            summary.join(" "),
        ))));
    }

    // Validate every slot_name against the SIG's abstract-type slots. The
    // `abstract_type_names_of` helper sweeps both the SIG's `bindings.types` (where
    // `LET Type = Number` lands post-stage-3) and `bindings.data` (lowercase abstract
    // fallback). Reject unknown names here so the sub-Dispatch path doesn't fire on
    // garbage slots.
    let known_slots: std::collections::HashSet<String> =
        abstract_type_names_of(s.decl_scope()).into_iter().collect();
    for (slot_name, _, _) in &triples {
        if !is_abstract_type_name(slot_name) {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "SIG_WITH binding name `{slot_name}` does not classify as an abstract-type slot \
                 (Capital-first with a lowercase letter)",
            ))));
        }
        if !known_slots.contains(slot_name) {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "{} has no abstract type slot `{}`",
                s.path, slot_name,
            ))));
        }
    }

    // First pass: try synchronous elaboration. Bare `Type(t)` parts route through
    // `elaborate_type_expr`; `Future(KTypeValue)` carriers (Combine re-walk) lift
    // directly. Inner `Expression(...)` parts need a sub-Dispatch and force the
    // Combine path.
    let mut pinned: Vec<(String, KType)> = Vec::with_capacity(triples.len());
    let mut sub_dispatches: Vec<(usize, crate::ast::KExpression<'a>)> = Vec::new();
    let mut placeholders: Vec<usize> = Vec::new(); // indices in `pinned` for sub-dispatch slots
    for (slot_name, value_part, _idx) in &triples {
        match value_part {
            ExpressionPart::Type(t) => {
                let mut el = Elaborator::new(scope);
                match elaborate_type_expr(&mut el, t) {
                    ElabResult::Done(kt) => pinned.push((slot_name.clone(), kt)),
                    ElabResult::Park(_) => {
                        // Treat a parked leaf as a sub-Dispatch on the bare leaf:
                        // wrap it in a one-part Expression that the value_lookup
                        // overload resolves to a `Future(KTypeValue(_))`. Today the
                        // SIG_WITH-at-FN-slot path already runs after the outer
                        // FN-def's Combine wake when leaves have terminalized, so
                        // this is a defensive fallback rather than the hot path.
                        return err(KError::new(KErrorKind::ShapeError(format!(
                            "SIG_WITH binding `{slot_name}` parked on unresolved leaf \
                             `{}` — Phase A1 requires synchronously-resolvable leaves",
                            t.render(),
                        ))));
                    }
                    ElabResult::Unbound(msg) => {
                        return err(KError::new(KErrorKind::ShapeError(format!(
                            "SIG_WITH binding `{slot_name}` value: {msg}",
                        ))));
                    }
                }
            }
            ExpressionPart::Future(KObject::KTypeValue(kt)) => {
                pinned.push((slot_name.clone(), kt.clone()));
            }
            ExpressionPart::Future(other) => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "SIG_WITH binding `{slot_name}` value must be a type expression, got a `{}` value",
                    other.ktype().name(),
                ))));
            }
            ExpressionPart::Expression(boxed) => {
                // Sub-Dispatch the inner parens expression on the scheduler. Reserve
                // a placeholder slot in `pinned` so the slot-name order is stable;
                // the Combine finish overwrites it with the resolved `KType`.
                let placeholder_idx = pinned.len();
                pinned.push((slot_name.clone(), KType::Any));
                placeholders.push(placeholder_idx);
                sub_dispatches.push((placeholder_idx, (**boxed).clone()));
            }
            other => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "SIG_WITH binding `{slot_name}` value must be a type expression \
                     (Type token, parens-wrapped expression, or already-resolved Future), \
                     got `{}`",
                    other.summarize(),
                ))));
            }
        }
    }

    let sig_id = s.sig_id();
    let sig_path = s.path.clone();

    if sub_dispatches.is_empty() {
        // Fully synchronous path — alloc the SignatureBound carrier directly.
        return BodyResult::Value(
            scope.arena.alloc_object(KObject::KTypeValue(KType::SignatureBound {
                sig_id,
                sig_path,
                pinned_slots: pinned,
            })),
        );
    }

    // Combine path. Schedule each sub-Dispatch, collect their NodeIds in submission
    // order, then build a Combine whose finish re-reads each result as a
    // `KObject::KTypeValue(kt)` and overwrites the placeholder at `pinned[idx]`.
    let mut deps: Vec<crate::runtime::machine::NodeId> = Vec::with_capacity(sub_dispatches.len());
    // `submission_layout[k] = pinned_idx` says "results[k] fills pinned[pinned_idx]".
    let mut submission_layout: Vec<usize> = Vec::with_capacity(sub_dispatches.len());
    for (pinned_idx, sub_expr) in sub_dispatches {
        deps.push(sched.add_dispatch(sub_expr, scope));
        submission_layout.push(pinned_idx);
    }

    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
        let mut pinned = pinned;
        for (k, &pinned_idx) in submission_layout.iter().enumerate() {
            let obj = results[k];
            match obj {
                KObject::KTypeValue(kt) => {
                    pinned[pinned_idx].1 = kt.clone();
                }
                other => {
                    return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                        "SIG_WITH binding `{}` value resolved to non-type `{}`",
                        pinned[pinned_idx].0,
                        other.ktype().name(),
                    ))));
                }
            }
        }
        BodyResult::Value(
            scope.arena.alloc_object(KObject::KTypeValue(KType::SignatureBound {
                sig_id,
                sig_path,
                pinned_slots: pinned,
            })),
        )
    });
    let combine_id = sched.add_combine(deps, scope, finish);
    BodyResult::DeferTo(combine_id)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "LIST_OF",
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::TypeExprRef),
            elements: vec![
                SignatureElement::Keyword("LIST_OF".into()),
                SignatureElement::Argument(Argument { name: "elem".into(), ktype: KType::TypeExprRef }),
            ],
        },
        body_list_of,
    );
    register_builtin(
        scope,
        "DICT_OF",
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::TypeExprRef),
            elements: vec![
                SignatureElement::Keyword("DICT_OF".into()),
                SignatureElement::Argument(Argument { name: "key".into(),   ktype: KType::TypeExprRef }),
                SignatureElement::Argument(Argument { name: "value".into(), ktype: KType::TypeExprRef }),
            ],
        },
        body_dict_of,
    );
    register_builtin(
        scope,
        "FUNCTION_OF",
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::TypeExprRef),
            elements: vec![
                SignatureElement::Keyword("FUNCTION_OF".into()),
                SignatureElement::Argument(Argument { name: "args".into(), ktype: KType::KExpression }),
                SignatureElement::Keyword("->".into()),
                SignatureElement::Argument(Argument { name: "ret".into(),  ktype: KType::TypeExprRef }),
            ],
        },
        body_function_of,
    );
    // Single overload: the `m` slot is `Module`. Bare Type-token operands
    // (`MODULE_TYPE_OF Foo Type`) ride the unified auto-wrap path and resolve through the
    // `value_lookup`-TypeExprRef overload to a `Future(KModule)`, which then matches this
    // slot strictly. Same shape as the ascription operators — no parallel TypeExprRef-lhs
    // overload needed.
    register_builtin(
        scope,
        "MODULE_TYPE_OF",
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::TypeExprRef),
            elements: vec![
                SignatureElement::Keyword("MODULE_TYPE_OF".into()),
                SignatureElement::Argument(Argument {
                    name: "m".into(),
                    ktype: KType::AnyUserType { kind: UserTypeKind::Module },
                }),
                SignatureElement::Argument(Argument { name: "name".into(), ktype: KType::TypeExprRef }),
            ],
        },
        body_module_type_of,
    );
    // `TYPE_CONSTRUCTOR <param:TypeExprRef>` — declares a higher-kinded type-constructor
    // slot (template form). Inside a SIG body, `LET Wrap = (TYPE_CONSTRUCTOR Type)` binds
    // `Wrap` to a `KTypeValue(UserType { kind: TypeConstructor { param_names: ["T"] }, .. })`
    // template; `ascribe.rs:body_opaque` re-mints the slot with a fresh per-call
    // `scope_id` and the slot's declared name (e.g. `Wrap`) on opaque ascription.
    register_builtin(
        scope,
        "TYPE_CONSTRUCTOR",
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::TypeExprRef),
            elements: vec![
                SignatureElement::Keyword("TYPE_CONSTRUCTOR".into()),
                SignatureElement::Argument(Argument { name: "param".into(), ktype: KType::TypeExprRef }),
            ],
        },
        body_type_constructor,
    );
    // `SIG_WITH <sig:Signature> <bindings:KExpression>` — see [`body_sig_with`] for the
    // inner-triple parsing rules. The `bindings` slot is `KExpression` (lazy), so the
    // dispatcher hands the parens group to the body verbatim; sub-Dispatch of inner
    // value expressions (`(Elt: (MODULE_TYPE_OF E Type))`) is the body's responsibility.
    register_builtin(
        scope,
        "SIG_WITH",
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::TypeExprRef),
            elements: vec![
                SignatureElement::Keyword("SIG_WITH".into()),
                SignatureElement::Argument(Argument {
                    name: "sig".into(),
                    ktype: KType::Signature,
                }),
                SignatureElement::Argument(Argument {
                    name: "bindings".into(),
                    ktype: KType::KExpression,
                }),
            ],
        },
        body_sig_with,
    );
}

#[cfg(test)]
mod tests {
    use crate::runtime::builtins::test_support::{parse_one, run, run_one, run_root_silent};
    use crate::runtime::model::{KObject, KType};
    use crate::runtime::machine::RuntimeArena;
    use crate::runtime::machine::execute::Scheduler;

    /// `(LIST_OF Number)` dispatches and produces a `KTypeValue` carrying the elaborated
    /// `KType::List(Number)` directly — no surface-form round-trip needed.
    #[test]
    fn list_of_number_lowers_to_list_number() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("LIST_OF Number"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(*kt, KType::List(Box::new(KType::Number)));
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// `(DICT_OF Str Number)` lowers to `Dict<Str, Number>`.
    #[test]
    fn dict_of_str_number_lowers_to_dict() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("DICT_OF Str Number"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::Dict(Box::new(KType::Str), Box::new(KType::Number))
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// Nested dispatch: `(LIST_OF (LIST_OF Number))` schedules the inner LIST_OF as a
    /// sub-Dispatch and the outer Bind splices the result in. End-to-end exercises the
    /// scheduler-driven type-expression path.
    #[test]
    fn nested_list_of_dispatches_through_scheduler() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("LIST_OF (LIST_OF Number)"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::List(Box::new(KType::List(Box::new(KType::Number))))
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// `(MODULE_TYPE_OF M Type)` reads the `Type` slot from a module's `type_members`
    /// table. Sets up an opaquely-ascribed module so `Type` is bound, then verifies the
    /// builtin returns a `KTypeValue` whose `KType::UserType { kind: Module, .. }`
    /// carries the abstract type's identity.
    #[test]
    fn module_type_of_resolves_via_module_member() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = ((LET Type = Number) (LET compare = 0))\n\
             SIG OrderedSig = ((LET Type = Number) (LET compare = 0))\n\
             LET Mod = (IntOrd :| OrderedSig)",
        );
        let result = run_one(scope, parse_one("MODULE_TYPE_OF Mod Type"));
        match result {
            KObject::KTypeValue(kt) => {
                // The abstract type member is recorded as `KType::UserType { kind:
                // Module, .. }` by the ascription path; surface name is `Type`.
                assert_eq!(kt.name(), "Type");
                use crate::runtime::model::types::UserTypeKind;
                assert!(matches!(
                    kt,
                    KType::UserType { kind: UserTypeKind::Module, .. }
                ));
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// MODULE_TYPE_OF on a module without that abstract member produces a clean
    /// `ShapeError` naming the module and the missing member.
    #[test]
    fn module_type_of_unknown_member_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "MODULE Foo = (LET x = 1)");
        // `Foo` is a Type token; the TypeExprRef-lhs overload looks it up against the
        // surrounding scope. `Bogus` is also a Type token naming a nonexistent abstract
        // member.
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("MODULE_TYPE_OF Foo Bogus"), scope);
        sched.execute().expect("scheduler runs to completion");
        let res = sched.read_result(id);
        assert!(res.is_err(), "expected MODULE_TYPE_OF on missing member to err");
    }

    /// `(SIG_WITH OrderedSig ((Type: Number)))` dispatches and returns a `KTypeValue`
    /// whose `SignatureBound` carries the matching `sig_id` and a one-entry
    /// `pinned_slots` vec pinning `Type` to `Number`.
    #[test]
    fn sig_with_one_slot_returns_signature_bound_with_pinned_slot() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG OrderedSig = ((LET Type = Number) (LET compare = 0))");
        // Pull the SIG's sig_id out of the scope so we can compare.
        let sig_id = match scope.bindings().data().get("OrderedSig") {
            Some(KObject::KSignature(s)) => s.sig_id(),
            _ => panic!("OrderedSig must bind a KSignature"),
        };
        let result = run_one(scope, parse_one("SIG_WITH OrderedSig ((Type: Number))"));
        match result {
            KObject::KTypeValue(kt) => match kt {
                KType::SignatureBound { sig_id: id, sig_path, pinned_slots } => {
                    assert_eq!(*id, sig_id);
                    assert_eq!(sig_path, "OrderedSig");
                    assert_eq!(pinned_slots.len(), 1);
                    assert_eq!(pinned_slots[0].0, "Type");
                    assert_eq!(pinned_slots[0].1, KType::Number);
                }
                other => panic!("expected SignatureBound, got {:?}", other),
            },
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// Multi-slot `(SIG_WITH Set ((Elt: Number) (Ord: Number)))` returns the pins in
    /// source order — `pinned_slots` is an order-preserving `Vec`, not a `HashMap`.
    /// (Using `Number` for both slots since the test only cares about ordering, not
    /// pinning to a custom type.)
    #[test]
    fn sig_with_two_slots_preserves_source_order() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG Set = ((LET Elt = Number) (LET Ord = Number) (LET tag = 0))",
        );
        let result = run_one(scope, parse_one("SIG_WITH Set ((Elt: Number) (Ord: Str))"));
        match result {
            KObject::KTypeValue(KType::SignatureBound { pinned_slots, .. }) => {
                assert_eq!(pinned_slots.len(), 2);
                assert_eq!(pinned_slots[0].0, "Elt");
                assert_eq!(pinned_slots[0].1, KType::Number);
                assert_eq!(pinned_slots[1].0, "Ord");
                assert_eq!(pinned_slots[1].1, KType::Str);
            }
            other => panic!("expected SignatureBound KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// Inner `MODULE_TYPE_OF` reference in a pin-value position sub-dispatches and the
    /// resulting `pinned_slots` carries the `KType::UserType { kind: Module, .. }` minted
    /// by ascription. Exercises the body's Combine-on-sub-dispatches path.
    #[test]
    fn sig_with_inner_module_attr_path_elaborates() {
        use crate::runtime::model::types::UserTypeKind;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        // Use a multi-letter Type-token name for the ascribed module so it classifies as
        // a Type (single-letter `E` is reserved by koan's token-classification rules —
        // see `tokens.rs`).
        run(
            scope,
            "MODULE IntOrd = ((LET Type = Number) (LET compare = 0))\n\
             SIG OrderedSig = ((LET Type = Number) (LET compare = 0))\n\
             SIG SetSig = ((LET Elt = Number) (LET insert = 0))\n\
             LET Elem = (IntOrd :| OrderedSig)",
        );
        let result = run_one(
            scope,
            parse_one("SIG_WITH SetSig ((Elt: (MODULE_TYPE_OF Elem Type)))"),
        );
        match result {
            KObject::KTypeValue(KType::SignatureBound { sig_path, pinned_slots, .. }) => {
                assert_eq!(sig_path, "SetSig");
                assert_eq!(pinned_slots.len(), 1);
                assert_eq!(pinned_slots[0].0, "Elt");
                match &pinned_slots[0].1 {
                    KType::UserType { kind: UserTypeKind::Module, name, .. } => {
                        assert_eq!(name, "Type");
                    }
                    other => panic!(
                        "expected pinned Elt to be UserType(Module, Type), got {:?}",
                        other,
                    ),
                }
            }
            other => panic!("expected SignatureBound KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// Unknown abstract-type slot names error with a focused diagnostic naming the SIG
    /// and the offending name.
    #[test]
    fn sig_with_rejects_unknown_slot() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG OrderedSig = ((LET Type = Number) (LET compare = 0))");
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("SIG_WITH OrderedSig ((Bogus: Number))"), scope);
        sched.execute().expect("scheduler runs to completion");
        let err = match sched.read_result(id) {
            Ok(_) => panic!("SIG_WITH on unknown slot must err"),
            Err(e) => e.clone(),
        };
        let msg = format!("{}", err);
        assert!(
            msg.contains("OrderedSig") && msg.contains("Bogus"),
            "expected error to name both SIG and slot, got: {msg}",
        );
    }

    /// Lowercase slot names (`type` instead of `Type`) parse as `Identifier`, not
    /// `Type`, and SIG_WITH rejects them with the abstract-type classification rule.
    #[test]
    fn sig_with_rejects_identifier_slot_name() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG OrderedSig = ((LET Type = Number) (LET compare = 0))");
        let mut sched = Scheduler::new();
        // `type` is an Identifier (lowercase first letter). The body rejects this
        // before the abstract-type-slot lookup, so the error names the classification
        // rule.
        let id = sched.add_dispatch(parse_one("SIG_WITH OrderedSig ((type: Number))"), scope);
        sched.execute().expect("scheduler runs to completion");
        let err = match sched.read_result(id) {
            Ok(_) => panic!("SIG_WITH with lowercase slot must err"),
            Err(e) => e.clone(),
        };
        let msg = format!("{}", err);
        assert!(
            msg.contains("Type token") || msg.contains("Capital-first"),
            "expected diagnostic to mention the Type-classification rule, got: {msg}",
        );
    }

    /// `(SIG_WITH OrderedSig Type Number)` — without parens around the binding triples
    /// — errors before validating slot names. The `bindings` slot is a KExpression
    /// whose parts must each be `Expression(...)`; bare `Type(...)` tokens at the top
    /// level get a focused rejection.
    ///
    /// Important caveat: the dispatcher will only reach `body_sig_with` if the slot
    /// signature actually matches. `SIG_WITH OrderedSig Type Number` is a 4-part
    /// expression where `Type` is a Type-token at part index 2. The `bindings` slot is
    /// at index 2 and accepts `KExpression`, which `Type` is not — so the dispatcher
    /// fails before the body fires. Wrap the malformed bindings in a single parens to
    /// route through the body and exercise its rejection path.
    #[test]
    fn sig_with_rejects_non_parens_bindings_form() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG OrderedSig = ((LET Type = Number) (LET compare = 0))");
        // `(Type Number)` is a single Expression at the bindings slot — its parts
        // are bare `Type(Type)`, `Type(Number)`, neither wrapped in their own
        // parens. The body should reject because the top-level parts inside the
        // bindings group must each be `Expression(...)`.
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("SIG_WITH OrderedSig (Type Number)"), scope);
        sched.execute().expect("scheduler runs to completion");
        let err = match sched.read_result(id) {
            Ok(_) => panic!("malformed bindings form must err"),
            Err(e) => e.clone(),
        };
        let msg = format!("{}", err);
        assert!(
            msg.contains("SIG_WITH bindings") || msg.contains("parens-wrapped"),
            "expected diagnostic to mention the bindings shape, got: {msg}",
        );
    }

    // ---------- Module-system stage 2 Workstream B: TYPE_CONSTRUCTOR builtin ----------

    /// `(TYPE_CONSTRUCTOR Type)` returns a `KTypeValue` wrapping a template
    /// `KType::UserType { kind: UserTypeKind::TypeConstructor { param_names: ["T"] }, .. }`
    /// with the sentinel placeholder name (`_typeconstructor`) and `scope_id: 0`. The
    /// ascription site re-mints with the slot's declared name and a fresh per-call
    /// `scope_id`; this test just pins the template shape the builtin returns.
    #[test]
    fn type_constructor_builtin_returns_ktype_value() {
        use crate::runtime::model::types::UserTypeKind;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("TYPE_CONSTRUCTOR Type"));
        match result {
            KObject::KTypeValue(kt) => match kt {
                KType::UserType { kind: UserTypeKind::TypeConstructor { param_names }, scope_id, name } => {
                    assert_eq!(*param_names, vec!["Type".to_string()]);
                    assert_eq!(*scope_id, 0);
                    assert_eq!(name, "_typeconstructor");
                }
                other => panic!("expected UserType(TypeConstructor), got {:?}", other),
            },
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// `SIG Monad = ((LET Wrap = (TYPE_CONSTRUCTOR Type)))` parses and binds. The SIG's
    /// decl-scope carries a `KType::UserType { kind: TypeConstructor { .. }, .. }` template
    /// in `bindings.types` under `Wrap`. Pins the LET-routing + register_type path.
    #[test]
    fn sig_declares_higher_kinded_slot() {
        use crate::runtime::model::types::UserTypeKind;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG Monad = ((LET Wrap = (TYPE_CONSTRUCTOR Type)))");
        let s = match scope.bindings().data().get("Monad") {
            Some(KObject::KSignature(s)) => *s,
            _ => panic!("Monad must bind a KSignature"),
        };
        let decl_types = s.decl_scope().bindings().types();
        let wrap_kt: &KType = decl_types.get("Wrap").copied().expect("Wrap must live in types");
        match wrap_kt {
            KType::UserType { kind: UserTypeKind::TypeConstructor { param_names }, .. } => {
                assert_eq!(*param_names, vec!["Type".to_string()]);
            }
            other => panic!("expected UserType(TypeConstructor) under Wrap, got {:?}", other),
        }
    }

    /// Module-system stage 2 Workstream B2: end-to-end smoke test for the monad-shaped
    /// signature. `SIG Monad = ((LET Wrap = (TYPE_CONSTRUCTOR Type)) (LET pure = (FN
    /// (PURE a: Number) -> Wrap<Number> = 1)))` parses, the SIG body's FN-def
    /// elaborates `Wrap<Number>` through the new `ConstructorApply` arm in
    /// `elaborate_type_expr`, and the resulting `pure` member is bound under the
    /// SIG's decl-scope. Load-bearing for `monadic-side-effects.md`.
    ///
    /// Fn-def whose return type is `Wrap<Number>` against a root-scope-bound
    /// TypeConstructor `Wrap`. Pins the dispatch path: `resolve_for` turns the
    /// parameterized type into a `TypeNameRef` carrier, `elaborate_type_expr` runs
    /// the new ConstructorApply arm, and the FN's stored signature carries a
    /// `KType::ConstructorApply { ctor: Wrap, args: [Number] }`. Isolates the path
    /// from SIG-body forward-reference parking (covered by `monad_signature_smoke`).
    #[test]
    fn fn_return_type_constructor_apply_root_scope() {
        use crate::runtime::model::types::UserTypeKind;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        scope.register_type(
            "Wrap".into(),
            KType::UserType {
                kind: UserTypeKind::TypeConstructor { param_names: vec!["Type".into()] },
                scope_id: 0xC0DE,
                name: "Wrap".into(),
            },
        );
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(
            parse_one("LET pure = (FN (PURE a: Number) -> Wrap<Number> = (1))"),
            scope,
        );
        sched.execute().expect("scheduler should run");
        match sched.read_result(id) {
            Ok(_) => {}
            Err(e) => panic!("FN with Wrap<Number> return failed: {}", e),
        }
        // Verify the FN's return type is ConstructorApply<Wrap, [Number]>.
        let pure = scope.bindings().data().get("pure").copied().expect("pure bound");
        let f = match pure {
            KObject::KFunction(f, _) => *f,
            other => panic!("pure not KFunction: {:?}", other.ktype()),
        };
        use crate::runtime::model::ReturnType;
        match &f.signature.return_type {
            ReturnType::Resolved(KType::ConstructorApply { args, .. }) => {
                assert_eq!(*args, vec![KType::Number]);
            }
            other => panic!("expected Resolved(ConstructorApply), got {:?}", other),
        }
    }

    /// Module-system stage 2 Workstream B2: end-to-end smoke test for the monad-shaped
    /// signature. `SIG Monad = ((LET Wrap = (TYPE_CONSTRUCTOR Type)) (LET pure = (FN
    /// (PURE a: Number) -> Wrap<Number> = 1)))` parses, the SIG body's FN-def
    /// elaborates `Wrap<Number>` through the new `ConstructorApply` arm in
    /// `elaborate_type_expr`, and the resulting `pure` member is bound under the
    /// SIG's decl-scope. Load-bearing for `monadic-side-effects.md`.
    ///
    /// `Number` is used as the parameter type rather than `T` because koan's token
    /// classification rejects single-letter Type tokens (needs ≥1 lowercase). The
    /// roadmap-decided surface form `(TYPE_CONSTRUCTOR T)` is conceptual; the runtime
    /// param symbol is whatever Type-classified token the user writes (here `Type`,
    /// a builtin meta-type name).
    #[test]
    fn monad_signature_smoke() {
        use crate::runtime::model::types::UserTypeKind;
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        // FN body must be a parens-wrapped expression — `KType::KExpression` slot
        // requires `ExpressionPart::Expression(_)`, not a bare `Literal`. Use `(1)`
        // accordingly so the dispatch reaches `body_fn`.
        let src = "SIG Monad = ((LET Wrap = (TYPE_CONSTRUCTOR Type)) \
             (LET pure = (FN (PURE a: Number) -> Wrap<Number> = (1))))";
        let exprs = parse(src).expect("parse should succeed");
        let mut sched = Scheduler::new();
        let mut ids = Vec::new();
        for expr in exprs {
            ids.push(sched.add_dispatch(expr, scope));
        }
        match sched.execute() {
            Ok(()) => {}
            Err(e) => panic!("scheduler errored: {}", e),
        }
        for (i, id) in ids.iter().enumerate() {
            if let Err(e) = sched.read_result(*id) {
                panic!("expr {} errored: {}", i, e);
            }
        }
        // The SIG must have bound — pull it out of scope and walk its decl_scope.
        let s = match scope.bindings().data().get("Monad") {
            Some(KObject::KSignature(s)) => *s,
            other => panic!("Monad must bind a KSignature, got {:?}", other.map(|o| o.ktype())),
        };
        // `Wrap` lives in the SIG's `bindings.types` as a TypeConstructor template.
        let decl_types = s.decl_scope().bindings().types();
        let wrap_kt: &KType = decl_types
            .get("Wrap")
            .copied()
            .expect("Wrap must live in SIG's types map");
        assert!(matches!(
            wrap_kt,
            KType::UserType { kind: UserTypeKind::TypeConstructor { .. }, .. }
        ));
        drop(decl_types);
        // `pure` is bound in the SIG's `bindings.data`; its FN signature's return type
        // must be a ConstructorApply (elaborated against the SIG body's decl-scope
        // where Wrap is in scope).
        let decl_data = s.decl_scope().bindings().data();
        let pure = decl_data.get("pure").copied().expect("pure must live in SIG's data");
        let f = match pure {
            KObject::KFunction(f, _) => *f,
            other => panic!("pure must be a KFunction, got {:?}", other.ktype()),
        };
        use crate::runtime::model::ReturnType;
        match &f.signature.return_type {
            ReturnType::Resolved(KType::ConstructorApply { ctor, args }) => {
                // The constructor is the SIG-body's template `Wrap` (scope_id 0, name
                // `_typeconstructor` per body_type_constructor's template form — note
                // the SIG body's `LET Wrap = ...` does NOT rebrand the template; the
                // re-mint happens at opaque ascription, not at SIG declaration). What
                // matters for the smoke is that the ConstructorApply was emitted at all
                // and carries the right structural shape.
                assert!(matches!(
                    ctor.as_ref(),
                    KType::UserType { kind: UserTypeKind::TypeConstructor { .. }, .. }
                ), "ConstructorApply.ctor must be a TypeConstructor, got {:?}", ctor);
                assert_eq!(*args, vec![KType::Number]);
            }
            other => panic!(
                "pure's return type must be Resolved(ConstructorApply), got {:?}",
                other,
            ),
        }
    }

    /// `(M.Wrap)` after opaque ascription resolves through the new module's
    /// `type_members` to the per-call-minted constructor variant. Pins the ATTR path's
    /// flow: `attr.rs` routes `Foo.Wrap` through `type_members` lookup, and the new
    /// `UserTypeKind::TypeConstructor` variant flows through the existing
    /// `KType::UserType` arm unchanged.
    /// Miri audit-slate: pins type-op dispatch through the per-call arena under tree
    /// borrows. A functor body invokes `(MODULE_TYPE_OF Er Type)` on its per-call
    /// parameter; `body_module_type_of` allocates the resulting `KTypeValue` into the
    /// per-call scope's arena. The returned `KModule` plus the bound type member must
    /// survive subsequent arena churn — the per-call-arena reclamation + lift machinery
    /// have to keep storage live for both the module pointer and the dispatched
    /// type-op value. Mirrors the structure of
    /// [`crate::runtime::builtins::fn_def::tests::module_stage2::functor_body_module_dispatch_does_not_dangle`]
    /// but pins the type-op-in-per-call-arena path rather than the plain functor lift.
    ///
    /// Module-system functor-params Stage B: parameter migrated from the lowercase
    /// workaround (`elem`) to the documented Type-class form (`Er`). Stage A's
    /// per-call dual-write makes the surface form work end-to-end through the
    /// signature-typed parameter path that previously parked on a missing top-level
    /// binding.
    #[test]
    fn type_op_dispatch_does_not_dangle() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = ((LET Type = Number) (LET compare = 0))\n\
             MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
             LET elem_mod = (IntOrd :| OrderedSig)",
        );
        // Functor body invokes MODULE_TYPE_OF on the per-call parameter `Er`. The
        // dispatched `KTypeValue` is allocated in the per-call arena and bound into the
        // result module via `LET Tslot` (uppercase — drives the LET TypeExprRef
        // overload that writes into `bindings.types`). A second plain `LET probe = 11`
        // gives a value-side binding to read back.
        run(
            scope,
            "FN (LIFT_TYPE Er: OrderedSig) -> Module = \
             (MODULE Result = ((LET Tslot = (MODULE_TYPE_OF Er Type)) (LET probe = 11)))",
        );
        run(scope, "LET held = (LIFT_TYPE (elem_mod))");

        // Subsequent allocations and FN calls churn the run-root arena. The lifted
        // `KModule` and its child scope (carrying the dispatched type-op value) must
        // survive that churn.
        run(scope, "FN (NOOP) -> Number = (1)");
        for _ in 0..20 {
            run_one(scope, parse_one("NOOP"));
        }
        // Another functor call to allocate more per-call frames (and drop them).
        run(scope, "LET other = (LIFT_TYPE (elem_mod))");

        // Hold the original `held` module across all that churn and read both surfaces
        // the audit pins: `child_scope()` (the captured-scope transmute) and
        // `type_members` (the RefCell on the Module).
        let data = scope.bindings().data();
        let m = match data.get("held") {
            Some(KObject::KModule(m, _)) => *m,
            other => panic!("held should be a module, got {:?}", other.map(|o| o.ktype())),
        };
        let probe = m.child_scope().bindings().data().get("probe").copied();
        assert!(
            matches!(probe, Some(KObject::Number(n)) if *n == 11.0),
            "held.probe must still read 11.0 after subsequent churn",
        );
        // `Tslot` landed in `bindings.types` via the LET TypeExprRef overload — the
        // dispatched `KTypeValue` from the per-call MODULE_TYPE_OF call.
        let tslot = m.child_scope().resolve_type("Tslot");
        assert!(
            tslot.is_some(),
            "held.Tslot must still resolve through bindings.types after churn",
        );
        // The RefCell on `type_members` is the other half of the Module's lifetime
        // surface; the borrow must complete cleanly (we don't assert contents here —
        // the body's module isn't opaquely ascribed, so type_members is empty).
        let _ = m.type_members.borrow();
    }

    #[test]
    fn module_attr_access_returns_type_constructor() {
        use crate::runtime::model::types::UserTypeKind;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG MonadSig = ((LET Wrap = (TYPE_CONSTRUCTOR Type)))\n\
             MODULE IntList = ((LET Wrap = Number))\n\
             LET Mo = (IntList :| MonadSig)",
        );
        // Mo's type_members must carry a TypeConstructor slot under `Wrap`.
        let mo = match scope.bindings().data().get("Mo") {
            Some(KObject::KModule(m, _)) => *m,
            other => panic!("Mo should be a module, got {:?}", other.map(|o| o.ktype())),
        };
        let wrap_t = mo.type_members.borrow().get("Wrap").cloned();
        match wrap_t {
            Some(KType::UserType { kind: UserTypeKind::TypeConstructor { param_names }, name, .. }) => {
                assert_eq!(name, "Wrap");
                // The per-call mint carries the SIG's declared param-name list.
                assert_eq!(param_names, vec!["Type".to_string()]);
            }
            other => panic!(
                "expected TypeConstructor in type_members[Wrap], got {:?}",
                other,
            ),
        }
    }
}
