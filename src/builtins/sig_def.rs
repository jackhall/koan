//! `SIG <name:ProperType> = <body:KExpression>` — declare a module signature (an
//! interface a module can be ascribed to). See
//! [design/typing/modules.md](../../design/typing/modules.md).
//!
//! Routes [`await_body_in_scope`](super::await_body::await_body_in_scope) like
//! `module_def` / `recursive_types`: body statements dispatch against a fresh child scope
//! on the outer scheduler, and the finish captures the populated scope into a
//! [`ModuleSignature`] value, allocates it in the parent's region, and binds it under the
//! signature's name. `VAL <name> :Type` declares a value slot, `TYPE <Name>` declares an
//! abstract type member, and `LET <Name> = <Type>` declares a manifest type member. The
//! ascription operators (`:|` / `:!`) iterate the stored scope at ascription time.

use crate::machine::model::KType;
use crate::machine::model::ModuleSignature;
use crate::machine::model::{KKind, SigSource};
use crate::machine::{Scope, TraceFrame};

use super::{arg, kw, sig};

/// The SIG body: mints the declaration scope, dispatches the SIG body block against it via
/// [`await_body_in_scope`](super::await_body::await_body_in_scope), and the finish captures
/// that scope into a [`ModuleSignature`] and installs the `KType::Signature` identity into
/// the parent scope.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use super::await_body::{await_body_in_scope, ChildScopeSeal};
    use crate::machine::{require_bare_type_name, require_kexpression, Action};

    let name = crate::try_action!(require_bare_type_name(ctx.args, "name", "SIG"));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "SIG", "body"));

    let decl_scope = ctx
        .scope
        .brand()
        .alloc_scope(Scope::child_under_sig(ctx.scope, name.clone()));

    let bind_index = ctx.bind_index();
    let name_for_finish = name;
    await_body_in_scope(
        decl_scope,
        body_expr,
        ChildScopeSeal::SealBeforeFinish,
        move |fctx| {
            let sig: &'a ModuleSignature<'a> = fctx
                .scope
                .brand()
                .alloc_signature(ModuleSignature::new(name_for_finish.clone(), decl_scope));
            let identity = KType::signature(SigSource::Declared(sig), Vec::new());
            match fctx
                .scope
                .register_nominal_upsert(name_for_finish.clone(), identity, bind_index)
            {
                Ok(kt_ref) => Action::Done(fctx.ctx.alloc_type_checked(kt_ref.clone())),
                Err(e) => Action::Done(Err(e.with_frame(TraceFrame::bare(
                    "<signature>",
                    format!("SIG {} body", name_for_finish),
                )))),
            }
        },
    )
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(
        KType::OfKind(KKind::Signature),
        vec![
            kw("SIG"),
            arg("name", KType::OfKind(KKind::ProperType)),
            kw("="),
            arg("body", KType::KExpression),
        ],
    );
    crate::builtins::register_builtin_full(
        scope,
        "SIG",
        signature,
        body,
        Some((super::type_part_binder_name, crate::machine::BindKind::Type)),
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::SigSource;
    use crate::builtins::test_support::{parse_one, run, run_one_err, run_root_silent};
    use crate::machine::run_root_storage;
    use crate::machine::KErrorKind;
    use crate::parse::parse;

    #[test]
    fn binder_name_extracts_sig_name() {
        let mut exprs = parse("SIG Ordered = (VAL x :Number)").expect("parse should succeed");
        let expr = exprs.remove(0);
        let name = expr.binder_name_from_type_part();
        assert_eq!(name.as_deref(), Some("Ordered"));
    }

    #[test]
    fn sig_binds_under_name_in_scope() {
        use crate::machine::model::KType;
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "SIG Ordered = (VAL x :Number)");
        // SIG installs a single type-side identity; nothing lands in `bindings.data`.
        assert!(scope.bindings().data().get("Ordered").is_none());
        assert!(matches!(
            scope.resolve_type("Ordered"),
            Some(KType::Signature { .. })
        ));
    }

    #[test]
    fn sig_path_records_name() {
        use crate::machine::model::KType;
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "SIG Ordered = (VAL x :Number)");
        let sig = match scope.resolve_type("Ordered") {
            Some(KType::Signature {
                sig: SigSource::Declared(sig),
                ..
            }) => *sig,
            _ => panic!("Ordered should be a signature"),
        };
        assert_eq!(sig.path, "Ordered");
    }

    /// Body-statement forward-reference: a SIG body's `VAL x :SomeType` parks on an
    /// outer-scope-bound type alias and resolves once the alias finalizes.
    #[test]
    fn sig_body_parks_on_outer_placeholder() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "LET MyAlias = Number\nSIG Foo = (VAL x :MyAlias)");
        use crate::machine::model::KType;
        let sig = match scope.resolve_type("Foo") {
            Some(KType::Signature {
                sig: SigSource::Declared(sig),
                ..
            }) => *sig,
            _ => panic!("Foo should be a signature"),
        };
        let x = sig
            .schema()
            .value_slots
            .get("x")
            .expect("VAL slot `x` must live in the signature's stored schema");
        assert!(
            matches!(x, KType::Number),
            "x's declared type must elaborate to Number through the alias, got {x:?}",
        );
    }

    /// A SIG-body abstract member named `Type` collides with the builtin `Type`
    /// meta-type: `TYPE Type` raises `Rebind` naming `Type`, the same unshadowable-builtins
    /// rule that gates a MODULE body's `LET Type`. Signatures name their principal abstract
    /// member `Carrier` (see [design/typing/modules.md](../../design/typing/modules.md)); this
    /// pins that the `Type` spelling does not declare a member.
    #[test]
    fn sig_member_named_type_collides_with_builtin_type() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let err = run_one_err(
            scope,
            parse_one("SIG Ordered = ((TYPE Type) (VAL compare :Number))"),
        );
        assert!(
            matches!(&err.kind, KErrorKind::Rebind { name } if name == "Type"),
            "a SIG member named `Type` must be a Rebind naming `Type`, got {err}",
        );
        assert!(
            scope.resolve_type("Ordered").is_none(),
            "the colliding signature binds nothing",
        );
    }

    /// A failing body statement surfaces as the SIG node's error and must not bind
    /// `Foo` (type side) in the parent scope.
    #[test]
    fn sig_body_error_short_circuits_finalize() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "SIG Foo = (VAL x :NonexistentType)");
        assert!(
            scope.resolve_type("Foo").is_none(),
            "Foo must not bind (type side) when its body errors",
        );
    }

    /// Content identity: two textually identical SIG declarations denote one type; a member whose
    /// type or name differs is a distinct type. Names bind the declaration, not the schema, so the
    /// differing binder names `Alpha`/`Beta` do not distinguish.
    #[test]
    fn identical_sigs_share_identity_differing_members_distinguish() {
        use crate::machine::model::KType;
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(
            scope,
            "SIG Alpha = ((VAL x :Number) (VAL y :Str))\n\
             SIG Beta = ((VAL x :Number) (VAL y :Str))\n\
             SIG Gamma = ((VAL x :Number) (VAL y :Bool))\n\
             SIG Delta = ((VAL x :Number) (VAL z :Str))",
        );
        let alpha = scope.resolve_type("Alpha").expect("Alpha binds");
        let beta = scope.resolve_type("Beta").expect("Beta binds");
        let gamma = scope.resolve_type("Gamma").expect("Gamma binds");
        let delta = scope.resolve_type("Delta").expect("Delta binds");
        assert!(matches!(alpha, KType::Signature { .. }));
        assert_eq!(alpha, beta, "identical schemas are one type");
        assert_ne!(alpha, gamma, "a differing member type distinguishes");
        assert_ne!(alpha, delta, "a differing member name distinguishes");
    }

    /// The binder-canonicalization trick: a value slot referencing the SIG's own abstract member
    /// digests as a name leaf, so two identical declarations unify despite distinct decl ids — and
    /// the self-reference digests differently from the same slot spelled with a manifest type.
    #[test]
    fn sig_self_referential_slot_canonicalizes() {
        use crate::machine::model::KType;
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(
            scope,
            "SIG OrdA = ((TYPE Elem) (VAL compare :(FN (a :Elem b :Elem) -> Bool)))\n\
             SIG OrdB = ((TYPE Elem) (VAL compare :(FN (a :Elem b :Elem) -> Bool)))\n\
             SIG OrdManifest = ((TYPE Elem) (VAL compare :(FN (a :Number b :Number) -> Bool)))",
        );
        let a = scope.resolve_type("OrdA").expect("OrdA binds");
        let b = scope.resolve_type("OrdB").expect("OrdB binds");
        let manifest = scope
            .resolve_type("OrdManifest")
            .expect("OrdManifest binds");
        assert!(matches!(a, KType::Signature { .. }));
        assert_eq!(
            a, b,
            "self-reference canonicalizes; identical declarations unify"
        );
        assert_ne!(
            a, manifest,
            "an abstract self-reference is not the same content as a manifest slot type",
        );
    }

    /// `WITH` pins fold into signature identity: differently-pinned views of one SIG are distinct,
    /// and a pinned view differs from the bare SIG. (Pin folding is unchanged by content identity —
    /// this guards it stays a content distinction.)
    #[test]
    fn with_pins_distinguish_signature_identity() {
        use crate::machine::model::KType;
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "SIG Container = ((TYPE Elem) (VAL item :Elem))");
        let container = match scope.resolve_type("Container") {
            Some(KType::Signature { sig, .. }) => *sig,
            _ => panic!("Container should be a signature"),
        };
        let pin_num = KType::signature(container, vec![("Elem".into(), KType::Number)]);
        let pin_str = KType::signature(container, vec![("Elem".into(), KType::Str)]);
        let bare = KType::signature(container, Vec::new());
        assert_ne!(pin_num, pin_str, "unequal pins are unequal types");
        assert_ne!(pin_num, bare, "a pin refines away from the bare signature");
    }
}
