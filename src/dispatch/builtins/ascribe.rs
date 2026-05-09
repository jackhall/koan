//! Ascription operators `:|` (opaque) and `:!` (transparent) — the module-system
//! mechanism for bolting a [`Signature`] onto a [`Module`]. See
//! [design/module-system.md](../../../design/module-system.md).
//!
//! Surface syntax:
//! ```text
//! LET IntOrdAbstract = (IntOrd :| OrderedSig)   -- opaque
//! LET IntOrdView     = (IntOrd :! OrderedSig)   -- transparent
//! ```
//!
//! Both operators consume `(Module, Signature)` and produce a `Module`. The pair was chosen
//! over `:>` / `<:` because the transparent `:!` differs from the opaque `:|` only by a
//! whitespace gap in the visual rendering, expressing "you can see through this."
//!
//! **Opaque ascription `:|`.** Mints a fresh `KType::ModuleType { scope_id, name }` for
//! every abstract type the signature declares. The new module's `child_scope` is a fresh
//! child of the parent scope (so two opaque ascriptions of the same source module yield
//! distinct module identities), populated with the source module's bindings as references
//! to the originals — opaque ascription doesn't deep-copy values, only their type identity
//! is reshaped. The `type_members` map is populated with the minted `ModuleType`s so
//! `IntOrd.Type` resolves to the abstract type rather than the underlying `Number`.
//!
//! **Transparent ascription `:!`.** Returns a "view" module that points at the same
//! `child_scope` as the source. Type identity is *not* reshaped — `IntOrdView.Type` still
//! resolves to whatever the source bound to `Type`. The work this still does is name- and
//! shape-checking: every operation declared in the signature must have a corresponding
//! binding in the source module. Mismatches are `ShapeError`s.
//!
//! Stage 1 limits the check to "every signature member has a source binding"; stricter
//! type-shape checks land alongside the inference scheduler in a later stage.

use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::dispatch::runtime::{KError, KErrorKind, Scope};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::values::{KObject, Module};

use super::register_builtin;

/// `<m:Module> :| <s:Signature>` — opaque ascription. Mints fresh `KType::ModuleType`s and
/// builds a new `Module` whose `child_scope` reuses the source module's bindings (by
/// reference) plus the new abstract-type table.
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

    // Re-bind every name from the source module into the new scope. The values themselves
    // are arena-allocated and immutable, so sharing the references is safe.
    let src = m.child_scope();
    for (name, obj) in src.data.borrow().iter() {
        new_scope.add(name.clone(), obj);
    }
    // Mirror the function-bucket entries too, so dispatch within the new module's child
    // scope sees the same overload set. Same reference-sharing rationale.
    for (key, bucket) in src.functions.borrow().iter() {
        new_scope
            .functions
            .borrow_mut()
            .entry(key.clone())
            .or_default()
            .extend(bucket.iter().copied());
    }

    let new_module: &'a Module<'a> = arena.alloc_module(Module::new(m.path.clone(), new_scope));
    // Mint fresh `ModuleType`s for every abstract-type declaration in the signature. The
    // surface convention is `LET <TypeName> = <expr>` inside the SIG body where the
    // `<TypeName>` classifies as a Type token (uppercase first + lowercase elsewhere).
    // Each minted `ModuleType` carries the new module's `scope_id`, so two distinct
    // opaque ascriptions yield distinct types — the abstraction-barrier identity property.
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

    // Stage 1: shape-check every signature operation has a binding in the source module.
    // Type-shape checks (full signature equivalence) are deferred.
    if let Err(e) = shape_check(s, src) {
        return BodyResult::Err(e);
    }

    let module_obj: &'a KObject<'a> = arena.alloc_object(KObject::KModule(new_module));
    BodyResult::Value(module_obj)
}

/// `<m:Module> :! <s:Signature>` — transparent ascription. Builds a view module pointing
/// at the source's `child_scope` (no fresh scope; type identity unchanged) and shape-checks
/// the source against the signature.
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
    // Transparent: reuse the source module's child scope verbatim — no fresh scope, no
    // remangled types. The new Module value just retags the path so error messages and
    // `summarize()` reflect that it's a view.
    let arena = scope.arena;
    let new_module: &'a Module<'a> = arena.alloc_module(Module::new(
        format!("{} :! {}", m.path, s.path),
        m.child_scope(),
    ));
    let module_obj: &'a KObject<'a> = arena.alloc_object(KObject::KModule(new_module));
    BodyResult::Value(module_obj)
}

/// Verify every operation name declared in `sig` has a binding in the source module's
/// `src_scope`. Abstract type declarations — bindings whose name classifies as a Type
/// token (uppercase first + at least one lowercase letter, per the [token classes in
/// design/type-system.md](../../../design/type-system.md#token-classes--the-parser-level-foundation))
/// — are abstraction shape, not implementation
/// requirements; the signature declares them to bind a name in the abstract type's slot,
/// and the source module isn't required to have a same-named binding. Stage 1 stops at
/// name-presence; full type-shape compatibility is deferred to a later inference-aware
/// stage.
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

/// True iff `name` would classify as a Type token at the parser per §2 — first char
/// uppercase + at least one lowercase letter elsewhere. Used by shape-check to skip
/// abstract-type declarations in a signature (`LET Type = Number`, `LET Elt = ...`).
fn is_abstract_type_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false; };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.any(|c| c.is_ascii_lowercase())
}

/// Resolve `m` and `s` from the bundle to a `(&Module, &Signature)` pair. Accepts either
/// the directly-typed `KObject::KModule` / `KObject::KSignature` arguments (when the lhs
/// /rhs are already evaluated module/signature values) or `KObject::TypeExprValue` tokens
/// that name the lookup target — `IntOrd :| OrderedSig` parses with both sides as Type
/// tokens per the §2 classification rule, and the lookup happens here.
fn resolve_module_and_signature<'a>(
    scope: &'a Scope<'a>,
    bundle: &ArgumentBundle<'a>,
) -> Result<(&'a crate::dispatch::values::Module<'a>, &'a crate::dispatch::values::Signature<'a>), KError> {
    let m = resolve_module(scope, bundle.get("m"), "m")?;
    let s = resolve_signature(scope, bundle.get("s"), "s")?;
    Ok((m, s))
}

fn resolve_module<'a>(
    scope: &'a Scope<'a>,
    obj: Option<&KObject<'a>>,
    arg: &str,
) -> Result<&'a crate::dispatch::values::Module<'a>, KError> {
    let Some(obj) = obj else {
        return Err(KError::new(KErrorKind::MissingArg(arg.to_string())));
    };
    if let Some(m) = obj.as_module() {
        return Ok(m);
    }
    if let Some(t) = obj.as_type_expr() {
        return match scope.lookup(&t.name) {
            Some(found) => found.as_module().ok_or_else(|| {
                KError::new(KErrorKind::TypeMismatch {
                    arg: arg.to_string(),
                    expected: "Module".to_string(),
                    got: found.ktype().name(),
                })
            }),
            None => Err(KError::new(KErrorKind::UnboundName(t.name.clone()))),
        };
    }
    Err(KError::new(KErrorKind::TypeMismatch {
        arg: arg.to_string(),
        expected: "Module".to_string(),
        got: obj.ktype().name(),
    }))
}

fn resolve_signature<'a>(
    scope: &'a Scope<'a>,
    obj: Option<&KObject<'a>>,
    arg: &str,
) -> Result<&'a crate::dispatch::values::Signature<'a>, KError> {
    let Some(obj) = obj else {
        return Err(KError::new(KErrorKind::MissingArg(arg.to_string())));
    };
    if let Some(s) = obj.as_signature() {
        return Ok(s);
    }
    if let Some(t) = obj.as_type_expr() {
        return match scope.lookup(&t.name) {
            Some(found) => found.as_signature().ok_or_else(|| {
                KError::new(KErrorKind::TypeMismatch {
                    arg: arg.to_string(),
                    expected: "Signature".to_string(),
                    got: found.ktype().name(),
                })
            }),
            None => Err(KError::new(KErrorKind::UnboundName(t.name.clone()))),
        };
    }
    Err(KError::new(KErrorKind::TypeMismatch {
        arg: arg.to_string(),
        expected: "Signature".to_string(),
        got: obj.ktype().name(),
    }))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Surface case: both sides are Type tokens (`IntOrd :| OrderedSig`). Per the token
    // classes in design/type-system.md module names always classify as Type, not
    // Identifier, so this is the overload that fires from user source. The body resolves
    // each token via `Scope::lookup` to its bound module/signature value.
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
    // Fallback: already-evaluated Module/Signature values (e.g., from a function returning
    // a module). The body's `resolve_module_and_signature` accepts both shapes.
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
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::runtime::{KErrorKind, RuntimeArena, Scope};
    use crate::dispatch::types::KType;
    use crate::dispatch::values::KObject;
    use crate::execute::scheduler::Scheduler;
    use crate::parse::expression_tree::parse;
    use crate::parse::kexpression::KExpression;

    struct SharedBuf(Rc<RefCell<Vec<u8>>>);
    impl Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    fn build_scope<'a>(arena: &'a RuntimeArena, captured: Rc<RefCell<Vec<u8>>>) -> &'a Scope<'a> {
        default_scope(arena, Box::new(SharedBuf(captured)))
    }

    fn parse_one(src: &str) -> KExpression<'static> {
        let mut exprs = parse(src).expect("parse should succeed");
        assert_eq!(exprs.len(), 1, "test helper expects a single expression");
        exprs.remove(0)
    }

    fn run<'a>(scope: &'a Scope<'a>, source: &str) {
        let exprs = parse(source).expect("parse should succeed");
        let mut sched = Scheduler::new();
        for expr in exprs {
            sched.add_dispatch(expr, scope);
        }
        sched.execute().expect("scheduler should succeed");
    }

    fn run_one<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> &'a KObject<'a> {
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().expect("scheduler should succeed");
        sched.read(id)
    }

    fn run_one_err<'a>(
        scope: &'a Scope<'a>,
        expr: KExpression<'a>,
    ) -> crate::dispatch::runtime::KError {
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().expect("scheduler should not surface errors directly");
        match sched.read_result(id) {
            Ok(_) => panic!("expected error"),
            Err(e) => e.clone(),
        }
    }

    #[test]
    fn opaque_ascription_returns_module() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
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
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
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
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
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
        // Two independent opaque ascriptions of the same source module mint distinct
        // `KType::ModuleType` values (different `scope_id`s) — the abstraction-barrier
        // identity property. Read the minted types out of each module's `type_members`.
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
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
        // Transparent ascription preserves the source's abstract-type definitions verbatim;
        // `type_members` stays empty (the source module has none).
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
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
        // The opaque module's child scope re-binds the source's members, so `IntOrdAbstract.compare`
        // resolves to whatever `IntOrd.compare` was.
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(
            scope,
            "MODULE IntOrd = (LET compare = 42)\n\
             SIG OrderedSig = (LET compare = 0)\n\
             LET IntOrdAbstract = (IntOrd :| OrderedSig)",
        );
        let result = run_one(scope, parse_one("IntOrdAbstract.compare"));
        assert!(matches!(result, KObject::Number(n) if *n == 42.0));
    }

    /// End-to-end design example. Exercises the full surface — `MODULE`, `SIG` with an
    /// abstract `Type` declaration, the `:|` opaque ascription, and `Foo.Type` retrieving
    /// the minted abstract type. Covers the user-visible workflow that
    /// [design/module-system.md](../../../design/module-system.md) describes.
    #[test]
    fn roadmap_example_int_ord_with_ordered_sig() {
        let arena = RuntimeArena::new();
        let captured = Rc::new(RefCell::new(Vec::new()));
        let scope = build_scope(&arena, captured);
        run(
            scope,
            "MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
             SIG OrderedSig = ((LET Type = Number) (LET compare = 0))\n\
             LET IntOrdAbstract = (IntOrd :| OrderedSig)",
        );

        let data = scope.data.borrow();
        // 1. The opaque ascription produced a Module, bound under the chosen name.
        let abstract_mod = match data.get("IntOrdAbstract") {
            Some(KObject::KModule(m)) => *m,
            other => panic!("IntOrdAbstract should be a module, got {:?}", other.map(|o| o.ktype())),
        };
        // 2. The minted ModuleType is distinct from the underlying Number.
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
        // 3. The module's operation members survived ascription — IntOrdAbstract.compare
        //    resolves to the underlying value.
        let compare = abstract_mod
            .child_scope()
            .data
            .borrow()
            .get("compare")
            .copied();
        assert!(matches!(compare, Some(KObject::Number(n)) if *n == 7.0));
    }
}
