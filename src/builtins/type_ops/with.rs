//! `<sig> WITH {<Slot> = <Type>, …}` — infix signature specialization. Pins a subset of
//! `sig`'s abstract-type slots, each to the type bound in the record literal, and **folds the
//! pins into the schema** ([`SigSchema::fold_pins`]): each pinned abstract member becomes a
//! manifest member, slot references to it substitute to the pinned type, and the folded schema
//! interns through the one signature constructor. Specialization therefore accumulates across
//! chained WITH, is order-independent, and introduces no second spelling —
//! `Ordered WITH {Carrier = Number}` is the same type as the SIG declaring
//! `Carrier = Number` outright. A pin naming a slot already fixed (a manifest member,
//! including one an earlier WITH folded) normalizes away when equal to the fixed type and is a
//! type error otherwise.
//!
//! The `{Slot = Type}` record literal eager-evaluates to a `KObject::Record` whose field
//! values are resolved `Held::Type`s — a dotted `er.Carrier` value sub-dispatches in value
//! context for free — so the body reads `(name, Held::Type)` entries directly: no lazy
//! binding slot, no `AwaitDeps`.

use std::collections::HashSet;

use crate::machine::model::{Held, KObject, KType, TypeNode};
use crate::machine::{KError, KErrorKind};

/// `<sig> WITH {<Slot> = <Type>, …}`: reads the `sig` type cell and the eager-evaluated `bindings`
/// record from `BodyCtx::args`, validates each pin against the SIG's abstract type slots, and
/// returns the specialized signature handle as a `Carried::Type`.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::{arg_held, arg_object, arg_type, Action};

    let done_err = |e: KError| Action::Done(Err(e));
    let mismatch = |got: String| {
        KError::new(KErrorKind::TypeMismatch {
            arg: "sig".to_string(),
            expected: "Signature".to_string(),
            got,
        })
    };
    let sig_handle = match arg_type(ctx.args, "sig") {
        Some(kt) => kt,
        None => match arg_held(ctx.args, "sig") {
            Some(Held::Object(object)) => {
                return done_err(mismatch(object.ktype().name(ctx.types)))
            }
            _ => return done_err(KError::new(KErrorKind::MissingArg("sig".to_string()))),
        },
    };
    let schema = match ctx.types.node(sig_handle) {
        TypeNode::Signature { schema, .. } => schema,
        _ => return done_err(mismatch(sig_handle.name(ctx.types))),
    };
    let fields = match arg_object(ctx.args, "bindings") {
        Some(KObject::Record(fields, _types)) => fields,
        _ => {
            return done_err(KError::new(KErrorKind::ShapeError(
                "WITH bindings must be a record literal `{Slot = Type, …}`".to_string(),
            )));
        }
    };
    // A binding names either an abstract slot (recorded as a pin) or a manifest member (its
    // type is already fixed). Slot names are capitalized, so a lowercase / unknown key is in
    // neither set; no separate name-shape check is needed.
    let abstract_slots: HashSet<String> = schema.abstract_members.keys().cloned().collect();
    let manifest_members: std::collections::HashMap<String, KType> = schema
        .manifest_members
        .iter()
        .map(|(n, t)| (n.clone(), *t))
        .collect();
    // Validation only: every pin must name a known slot and hold a type. A slot already fixed —
    // a manifest member, which is also what an earlier WITH's fold left behind — admits only an
    // equal re-pin, which normalizes away (added to `dropped`, never folded), so
    // `S WITH {Tag = Number}` and `(S WITH {A = Number}) WITH {A = Number}` keep their source's
    // signature identity; an unequal re-pin is a type error.
    let mut dropped: HashSet<String> = HashSet::new();
    for (name, value) in fields.iter() {
        let is_abstract = abstract_slots.contains(name);
        let manifest = manifest_members.get(name);
        if !is_abstract && manifest.is_none() {
            return done_err(KError::new(KErrorKind::ShapeError(format!(
                "{} has no abstract type slot `{name}`",
                sig_handle.name(ctx.types),
            ))));
        }
        let pin_type = match value {
            Held::Type(kt) => kt,
            Held::Object(other) => {
                return done_err(KError::new(KErrorKind::ShapeError(format!(
                    "WITH binding `{name}` value must be a type, got `{}`",
                    other.ktype().name(ctx.types),
                ))));
            }
            Held::UnresolvedType(ti) => {
                return done_err(KError::new(KErrorKind::UnboundName(ti.render())));
            }
        };
        if let Some(fixed) = manifest {
            if pin_type == fixed {
                dropped.insert(name.clone());
            } else {
                return done_err(KError::new(KErrorKind::ShapeError(format!(
                    "`{}.{name}` is a manifest type member fixed to `{}`; \
                     WITH cannot re-pin it to `{}`",
                    sig_handle.name(ctx.types),
                    fixed.render(ctx.types),
                    pin_type.render(ctx.types),
                ))));
            }
        }
    }

    let pins: Vec<(String, KType)> = fields
        .iter()
        .filter(|(name, _)| !dropped.contains(name.as_str()))
        .map(|(name, value)| match value {
            Held::Type(kt) => (name.clone(), *kt),
            Held::Object(_) | Held::UnresolvedType(_) => {
                unreachable!("validated above: every pin value is a type")
            }
        })
        .collect();
    let folded = schema.fold_pins(&pins, ctx.types);
    Action::Done(Ok(ctx.ctx.type_carried(ctx.types.signature(folded))))
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, TestRun};
    use crate::machine::model::{KType, TypeNode};
    use crate::machine::run_root_storage;

    /// `WITH` folds the pin into the schema: the abstract member becomes a manifest one fixed to
    /// the pinned type, so the specialized interface is a fully concrete schema, distinct from
    /// the bare (still-abstract) source.
    #[test]
    fn with_one_slot_folds_the_pin_into_the_schema() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("SIG Ordered = ((TYPE Carrier) (VAL compare :Number))");
        let bare = scope.resolve_type("Ordered").expect("Ordered binds");
        let result = test_run.run_one_type(parse_one("Ordered WITH {Carrier = Number}"));
        assert_ne!(result, bare, "a pin refines away from the bare signature");
        match test_run.types().node(result) {
            TypeNode::Signature { schema, .. } => {
                assert!(
                    schema.abstract_members.is_empty(),
                    "the pinned member is no longer abstract",
                );
                assert_eq!(schema.manifest_members.get("Carrier"), Some(&KType::NUMBER));
            }
            _ => panic!("expected Signature type, got {result:?}"),
        }
    }

    /// Folding substitutes slot references to the pinned member: `compare :Carrier` becomes
    /// `compare: Number` under `{Carrier = Number}`, so the specialized interface interns the
    /// same content as a SIG declaring the member manifest outright.
    #[test]
    fn with_fold_substitutes_slot_references_and_unifies_with_concrete_sig() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run(
            "SIG Ordered = ((TYPE Carrier) (VAL compare :Carrier))\n\
             SIG IntOrdered = ((LET Carrier = Number) (VAL compare :Carrier))",
        );
        let pinned = test_run.run_one_type(parse_one("Ordered WITH {Carrier = Number}"));
        match test_run.types().node(pinned) {
            TypeNode::Signature { schema, .. } => {
                assert_eq!(
                    schema.value_slots.get("compare"),
                    Some(&KType::NUMBER),
                    "the slot's reference to Carrier substitutes to the pinned type",
                );
            }
            _ => panic!("expected Signature type, got {pinned:?}"),
        }
        let concrete = scope.resolve_type("IntOrdered").expect("IntOrdered binds");
        assert_eq!(
            pinned, concrete,
            "a pinned interface and the equivalent concrete declaration are one type",
        );
    }

    /// Pin-set identity is order-independent: both members land as manifest schema entries, so
    /// either record-literal order interns the same specialized type.
    #[test]
    fn with_two_slots_fold_order_independently() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("SIG OrderedSet = ((TYPE Elt) (TYPE Ord) (VAL tag :Number))");
        let result = test_run.run_one_type(parse_one("OrderedSet WITH {Ord = Str, Elt = Number}"));
        match test_run.types().node(result) {
            TypeNode::Signature { schema, .. } => {
                assert_eq!(schema.manifest_members.get("Elt"), Some(&KType::NUMBER));
                assert_eq!(schema.manifest_members.get("Ord"), Some(&KType::STR));
            }
            _ => panic!("expected Signature type, got {result:?}"),
        }
        let literal_order =
            test_run.run_one_type(parse_one("OrderedSet WITH {Elt = Number, Ord = Str}"));
        assert_eq!(
            result, literal_order,
            "either literal order interns the same specialized type",
        );
    }

    /// Pins accumulate across chained WITH: each chaining order carries both pins and interns
    /// the same type as the one-shot form.
    #[test]
    fn with_pins_accumulate_across_chained_with() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run(
            "SIG OrderedSet = ((TYPE Elt) (TYPE Ord) (VAL tag :Number))\n\
             LET ByElt = (OrderedSet WITH {Elt = Number})\n\
             LET ByOrd = (OrderedSet WITH {Ord = Str})",
        );
        let both = test_run.run_one_type(parse_one("OrderedSet WITH {Elt = Number, Ord = Str}"));
        let elt_then_ord = test_run.run_one_type(parse_one("ByElt WITH {Ord = Str}"));
        let ord_then_elt = test_run.run_one_type(parse_one("ByOrd WITH {Elt = Number}"));
        assert_eq!(elt_then_ord, both, "chained WITH accumulates the first pin");
        assert_eq!(
            ord_then_elt, both,
            "accumulation is chaining-order-independent"
        );
    }

    /// An equal re-pin of an already-pinned slot normalizes away, keeping the source's identity;
    /// a conflicting re-pin is a type error, mirroring the manifest-member rule.
    #[test]
    fn with_repin_normalizes_when_equal_and_errors_when_conflicting() {
        use crate::machine::KErrorKind;
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run(
            "SIG OrderedSet = ((TYPE Elt) (TYPE Ord) (VAL tag :Number))\n\
             LET ByElt = (OrderedSet WITH {Elt = Number})",
        );
        let pinned = test_run.run_one_type(parse_one("OrderedSet WITH {Elt = Number}"));
        let repinned = test_run.run_one_type(parse_one("ByElt WITH {Elt = Number}"));
        assert_eq!(
            repinned, pinned,
            "an equal re-pin keeps the source identity"
        );
        let err = test_run.run_one_err(parse_one("ByElt WITH {Elt = Str}"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(m) if m.contains("manifest")),
            "a conflicting re-pin hits the manifest-fixity rule (the fold made the first pin \
             manifest), got {err}",
        );
    }

    /// A dotted `elem.Carrier` pin value sub-dispatches in value context to the abstract
    /// `Carrier` and folds into the schema's manifest members — a dotted pin value the keyworded record-literal
    /// handler could not take (was `#[ignore]`d there).
    #[test]
    fn with_inner_module_attr_path_pins_abstract_type() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run(
            "MODULE int_ord = ((LET Carrier = Number) (LET compare = 0))\n\
             SIG Ordered = ((TYPE Carrier) (VAL compare :Number))\n\
             SIG Set = ((TYPE Elt) (VAL insert :Number))\n\
             LET elem = (int_ord :| Ordered)",
        );
        let result = test_run.run_one_type(parse_one("Set WITH {Elt = elem.Carrier}"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::Signature { schema, .. } => {
                let elt = schema
                    .manifest_members
                    .get("Elt")
                    .expect("Elt folds to a manifest member");
                match types.node(*elt) {
                    TypeNode::AbstractType { name, .. } => assert_eq!(name, "Carrier"),
                    _ => panic!("expected Elt = AbstractType(Carrier), got {elt:?}"),
                }
            }
            _ => panic!("expected Signature type, got {result:?}"),
        }
    }

    #[test]
    fn with_rejects_unknown_slot() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("SIG Ordered = ((TYPE Carrier) (VAL compare :Number))");
        let runtime = &mut test_run.runtime;
        let id = runtime.dispatch_in_scope(parse_one("Ordered WITH {Bogus = Number}"), scope);
        runtime
            .execute()
            .expect("execute does not surface per-slot errors");
        match runtime.result_error(id) {
            Err(e) => assert!(
                format!("{e}").contains("no abstract type slot"),
                "expected unknown-slot rejection, got {e}",
            ),
            Ok(()) => panic!("WITH on unknown slot must err"),
        }
    }

    /// A pin equal to a manifest member's fixed type normalizes away: the schema stays
    /// empty and the resulting signature compares equal to the bare sig.
    #[test]
    fn with_equal_manifest_pin_normalizes_away() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("SIG Tagged = ((LET Tag = Number) (VAL value :Number))");
        let bare = scope
            .resolve_type("Tagged")
            .expect("Tagged must bind a Signature KType");
        let result = test_run.run_one_type(parse_one("Tagged WITH {Tag = Number}"));
        assert_eq!(
            result, bare,
            "an equal manifest pin must preserve signature identity"
        );
    }

    /// A pin unequal to a manifest member's fixed type is a manifest-fixity error.
    #[test]
    fn with_rejects_unequal_manifest_pin() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("SIG Tagged = ((LET Tag = Number) (VAL value :Number))");
        let runtime = &mut test_run.runtime;
        let id = runtime.dispatch_in_scope(parse_one("Tagged WITH {Tag = Str}"), scope);
        runtime
            .execute()
            .expect("execute does not surface per-slot errors");
        match runtime.result_error(id) {
            Err(e) => {
                let text = format!("{e}");
                assert!(
                    text.contains("Tag") && text.contains("manifest"),
                    "expected manifest-fixity rejection naming the slot, got {e}",
                );
            }
            Ok(()) => panic!("WITH re-pinning a manifest member to a different type must err"),
        }
    }

    /// A mixed record folds only the abstract slot; the equal manifest pin normalizes away,
    /// leaving the member exactly as declared.
    #[test]
    fn with_mixed_record_folds_only_abstract_pin() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("SIG Mixed = ((TYPE Elt) (LET Tag = Number) (VAL value :Number))");
        let result = test_run.run_one_type(parse_one("Mixed WITH {Elt = Str, Tag = Number}"));
        match test_run.types().node(result) {
            TypeNode::Signature { schema, .. } => {
                assert!(schema.abstract_members.is_empty());
                assert_eq!(schema.manifest_members.get("Elt"), Some(&KType::STR));
                assert_eq!(schema.manifest_members.get("Tag"), Some(&KType::NUMBER));
            }
            _ => panic!("expected Signature type, got {result:?}"),
        }
    }

    #[test]
    fn with_rejects_lowercase_slot_name() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("SIG Ordered = ((TYPE Carrier) (VAL compare :Number))");
        let runtime = &mut test_run.runtime;
        let id = runtime.dispatch_in_scope(parse_one("Ordered WITH {type = Number}"), scope);
        runtime
            .execute()
            .expect("execute does not surface per-slot errors");
        match runtime.result_error(id) {
            Err(e) => assert!(
                format!("{e}").contains("no abstract type slot"),
                "expected lowercase-slot rejection, got {e}",
            ),
            Ok(()) => panic!("WITH with lowercase slot must err"),
        }
    }
}
