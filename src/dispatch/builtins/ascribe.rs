//! Ascription operators `:|` (opaque) and `:!` (transparent) — bolt a [`Signature`] onto
//! a [`Module`]. Both consume `(Module, Signature)` and produce a `Module`.
//! See [design/module-system.md](../../../design/module-system.md).
//!
//! Stage 1 shape-checking is name-presence only; full type-shape checks are deferred to
//! the inference scheduler.

use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::dispatch::runtime::{KError, KErrorKind, Scope};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::values::{resolve_module, resolve_signature, KObject, Module};

use super::register_builtin;

/// `<m:Module> :| <s:Signature>` — opaque ascription.
pub fn body_opaque<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let (m, s) = match resolve_module_and_signature(scope, &bundle) {
        Ok(pair) => pair,
        Err(e) => return BodyResult::Err(e),
    };

    let arena = scope.arena;
    let new_scope = arena.alloc_scope(Scope::child_under_named(
        scope,
        format!("MODULE {} :| {}", m.path, s.path),
    ));

    // Mirror the source module's bindings into the new scope by reference (values are
    // arena-allocated and immutable). Direct insert bypasses `bind_value`/`register_function`
    // to avoid double-registering functions via both `data` and the bucket loop below.
    let src = m.child_scope();
    {
        let mut data = new_scope.data.borrow_mut();
        for (name, obj) in src.data.borrow().iter() {
            data.insert(name.clone(), obj);
        }
    }
    for (key, bucket) in src.functions.borrow().iter() {
        new_scope
            .functions
            .borrow_mut()
            .entry(key.clone())
            .or_default()
            .extend(bucket.iter().copied());
    }

    let new_module: &'a Module<'a> = arena.alloc_module(Module::new(m.path.clone(), new_scope));
    // Each minted `ModuleType` carries the new module's `scope_id`, so two opaque ascriptions
    // of the same source yield distinct types — the abstraction-barrier identity property.
    let scope_id = new_module.scope_id();
    let mut minted: Vec<(String, KType)> = Vec::new();
    for name in s.decl_scope().data.borrow().keys() {
        if is_abstract_type_name(name) {
            minted.push((
                name.clone(),
                KType::ModuleType { scope_id, name: name.clone() },
            ));
        }
    }
    if !minted.is_empty() {
        let mut tm = new_module.type_members.borrow_mut();
        for (n, t) in minted {
            tm.insert(n, t);
        }
    }

    if let Err(e) = shape_check(s, src) {
        return BodyResult::Err(e);
    }

    let module_obj: &'a KObject<'a> = arena.alloc_object(KObject::KModule(new_module));
    BodyResult::Value(module_obj)
}

/// `<m:Module> :! <s:Signature>` — transparent ascription.
pub fn body_transparent<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let (m, s) = match resolve_module_and_signature(scope, &bundle) {
        Ok(pair) => pair,
        Err(e) => return BodyResult::Err(e),
    };
    if let Err(e) = shape_check(s, m.child_scope()) {
        return BodyResult::Err(e);
    }
    // Reuse the source's child scope; the new Module just retags the path as a view.
    let arena = scope.arena;
    let new_module: &'a Module<'a> = arena.alloc_module(Module::new(
        format!("{} :! {}", m.path, s.path),
        m.child_scope(),
    ));
    let module_obj: &'a KObject<'a> = arena.alloc_object(KObject::KModule(new_module));
    BodyResult::Value(module_obj)
}

/// Verify every non-abstract-type name in `sig` has a binding in `src_scope`.
/// Abstract-type declarations are skipped: they shape the abstraction, not the implementation.
fn shape_check<'a>(
    sig: &crate::dispatch::values::Signature<'a>,
    src_scope: &Scope<'a>,
) -> Result<(), KError> {
    let sig_data = sig.decl_scope().data.borrow();
    let src_data = src_scope.data.borrow();
    for name in sig_data.keys() {
        if is_abstract_type_name(name) {
            continue;
        }
        if !src_data.contains_key(name) {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "module does not satisfy signature `{}`: missing member `{}`",
                sig.path, name
            ))));
        }
    }
    Ok(())
}

/// True iff `name` classifies as a Type token (first char uppercase + at least one
/// lowercase elsewhere). See [design/type-system.md](../../../design/type-system.md#token-classes--the-parser-level-foundation).
fn is_abstract_type_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false; };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.any(|c| c.is_ascii_lowercase())
}

/// Resolve `m` and `s` from the bundle. Accepts either already-evaluated `KModule` /
/// `KSignature` values or `TypeExprValue` tokens that name a lookup target.
fn resolve_module_and_signature<'a>(
    scope: &'a Scope<'a>,
    bundle: &ArgumentBundle<'a>,
) -> Result<(&'a crate::dispatch::values::Module<'a>, &'a crate::dispatch::values::Signature<'a>), KError> {
    let m_obj = bundle
        .get("m")
        .ok_or_else(|| KError::new(KErrorKind::MissingArg("m".to_string())))?;
    let s_obj = bundle
        .get("s")
        .ok_or_else(|| KError::new(KErrorKind::MissingArg("s".to_string())))?;
    let m = resolve_module(scope, m_obj, "m")?;
    let s = resolve_signature(scope, s_obj, "s")?;
    Ok((m, s))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Surface case: both sides are Type tokens (`IntOrd :| OrderedSig`). Module/signature
    // names always classify as Type, so this is the overload that fires from user source.
    register_builtin(
        scope,
        ":|",
        ExpressionSignature {
            return_type: KType::Module,
            elements: vec![
                SignatureElement::Argument(Argument { name: "m".into(), ktype: KType::TypeExprRef }),
                SignatureElement::Keyword(":|".into()),
                SignatureElement::Argument(Argument { name: "s".into(), ktype: KType::TypeExprRef }),
            ],
        },
        body_opaque,
    );
    register_builtin(
        scope,
        ":!",
        ExpressionSignature {
            return_type: KType::Module,
            elements: vec![
                SignatureElement::Argument(Argument { name: "m".into(), ktype: KType::TypeExprRef }),
                SignatureElement::Keyword(":!".into()),
                SignatureElement::Argument(Argument { name: "s".into(), ktype: KType::TypeExprRef }),
            ],
        },
        body_transparent,
    );
    // Fallback: already-evaluated Module/Signature values.
    register_builtin(
        scope,
        ":|",
        ExpressionSignature {
            return_type: KType::Module,
            elements: vec![
                SignatureElement::Argument(Argument { name: "m".into(), ktype: KType::Module }),
                SignatureElement::Keyword(":|".into()),
                SignatureElement::Argument(Argument { name: "s".into(), ktype: KType::Signature }),
            ],
        },
        body_opaque,
    );
    register_builtin(
        scope,
        ":!",
        ExpressionSignature {
            return_type: KType::Module,
            elements: vec![
                SignatureElement::Argument(Argument { name: "m".into(), ktype: KType::Module }),
                SignatureElement::Keyword(":!".into()),
                SignatureElement::Argument(Argument { name: "s".into(), ktype: KType::Signature }),
            ],
        },
        body_transparent,
    );
}

#[cfg(test)]
mod tests {
    use crate::dispatch::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::dispatch::runtime::{KErrorKind, RuntimeArena};
    use crate::dispatch::types::KType;
    use crate::dispatch::values::KObject;
    use crate::execute::scheduler::Scheduler;
    use crate::parse::expression_tree::parse;

    #[test]
    fn opaque_ascription_returns_module() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = (LET compare = 0)\n\
             SIG OrderedSig = (LET compare = 0)\n\
             LET IntOrdAbstract = (IntOrd :| OrderedSig)",
        );
        let data = scope.data.borrow();
        assert!(matches!(data.get("IntOrdAbstract"), Some(KObject::KModule(_))));
    }

    #[test]
    fn transparent_ascription_returns_module() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = (LET compare = 0)\n\
             SIG OrderedSig = (LET compare = 0)\n\
             LET IntOrdView = (IntOrd :! OrderedSig)",
        );
        let data = scope.data.borrow();
        assert!(matches!(data.get("IntOrdView"), Some(KObject::KModule(_))));
    }

    #[test]
    fn ascription_missing_member_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE Empty = (LET unrelated = 0)\n\
             SIG OrderedSig = (LET compare = 0)",
        );
        let err = run_one_err(scope, parse_one("Empty :| OrderedSig"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("OrderedSig") && msg.contains("`compare`")),
            "expected ShapeError naming OrderedSig and the missing member, got {err}",
        );
    }

    #[test]
    fn opaque_ascription_mints_distinct_module_type_per_application() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let src = "MODULE IntOrd = (LET compare = 0)\n\
             SIG OrderedSig = ((LET Type = Number) (LET compare = 0))\n\
             LET FirstAbstract = (IntOrd :| OrderedSig)\n\
             LET SecondAbstract = (IntOrd :| OrderedSig)";
        let exprs = parse(src).expect("parse should succeed");
        let mut sched = Scheduler::new();
        let mut ids = Vec::new();
        for expr in exprs {
            ids.push(sched.add_dispatch(expr, scope));
        }
        sched.execute().expect("scheduler should succeed");
        for (i, id) in ids.iter().enumerate() {
            if let Err(e) = sched.read_result(*id) {
                panic!("expr {} errored: {}", i, e);
            }
        }
        let data = scope.data.borrow();
        let a = match data.get("FirstAbstract") {
            Some(KObject::KModule(m)) => *m,
            _ => panic!("FirstAbstract should be a module"),
        };
        let b = match data.get("SecondAbstract") {
            Some(KObject::KModule(m)) => *m,
            _ => panic!("SecondAbstract should be a module"),
        };
        let a_t = a.type_members.borrow().get("Type").cloned();
        let b_t = b.type_members.borrow().get("Type").cloned();
        assert!(matches!(&a_t, Some(KType::ModuleType { .. })));
        assert!(matches!(&b_t, Some(KType::ModuleType { .. })));
        assert_ne!(a_t, b_t, "two opaque ascriptions must mint distinct ModuleTypes");
    }

    #[test]
    fn transparent_ascription_does_not_mint_module_types() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = (LET compare = 0)\n\
             SIG OrderedSig = (LET compare = 0)\n\
             LET ViewMod = (IntOrd :! OrderedSig)",
        );
        let data = scope.data.borrow();
        let v = match data.get("ViewMod") {
            Some(KObject::KModule(m)) => *m,
            _ => panic!("ViewMod should be a module"),
        };
        assert!(v.type_members.borrow().is_empty());
    }

    #[test]
    fn opaque_ascribed_module_member_access_works() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = (LET compare = 42)\n\
             SIG OrderedSig = (LET compare = 0)\n\
             LET IntOrdAbstract = (IntOrd :| OrderedSig)",
        );
        let result = run_one(scope, parse_one("IntOrdAbstract.compare"));
        assert!(matches!(result, KObject::Number(n) if *n == 42.0));
    }

    /// End-to-end example from [design/module-system.md](../../../design/module-system.md).
    #[test]
    fn roadmap_example_int_ord_with_ordered_sig() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
             SIG OrderedSig = ((LET Type = Number) (LET compare = 0))\n\
             LET IntOrdAbstract = (IntOrd :| OrderedSig)",
        );

        let data = scope.data.borrow();
        let abstract_mod = match data.get("IntOrdAbstract") {
            Some(KObject::KModule(m)) => *m,
            other => panic!("IntOrdAbstract should be a module, got {:?}", other.map(|o| o.ktype())),
        };
        let minted = abstract_mod
            .type_members
            .borrow()
            .get("Type")
            .cloned()
            .expect("opaque ascription should mint a Type member");
        match &minted {
            KType::ModuleType { name, .. } => assert_eq!(name, "Type"),
            other => panic!("minted abstract type must be ModuleType, got {:?}", other),
        }
        assert_ne!(minted, KType::Number, "opaque IntOrdAbstract.Type must not equal Number");
        let compare = abstract_mod
            .child_scope()
            .data
            .borrow()
            .get("compare")
            .copied();
        assert!(matches!(compare, Some(KObject::Number(n)) if *n == 7.0));
    }
}
