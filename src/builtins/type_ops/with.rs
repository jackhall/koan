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

use crate::machine::model::{Held, KObject, KType, TypeNode, TypeSymbol};
use crate::machine::model::{display_label, render_label};
use crate::machine::{KError, KErrorKind};
use crate::witnessed::BumpVec;

/// `<sig> WITH {<Slot> = <Type>, …}`: reads the `sig` type cell and the eager-evaluated `bindings`
/// record from `BodyCtx::args`, validates each pin against the SIG's abstract type slots, and
/// returns the specialized signature handle as a `Carried::Type`.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::Action;

    let done_err = |e: KError| Action::done(Err(e));
    let mismatch = |got: String| {
        KError::new(KErrorKind::TypeMismatch {
            arg: "sig".to_string(),
            expected: "Signature".to_string(),
            got,
        })
    };
    let sig_handle = match ctx.args.ktype(&super::SLOTS.sig) {
        Some(kt) => kt,
        None => match ctx.args.held(&super::SLOTS.sig) {
            Some(Held::Object(object)) => {
                return done_err(mismatch(object.ktype().name(ctx.registries)));
            }
            _ => return done_err(KError::new(KErrorKind::MissingArg("sig".to_string()))),
        },
    };
    let schema = match ctx.types().node(sig_handle) {
        TypeNode::Signature { schema, .. } => schema,
        _ => return done_err(mismatch(sig_handle.name(ctx.registries))),
    };
    let bindings = match ctx.args.object(&super::SLOTS.bindings) {
        Some(KObject::Record(substrate, _types)) => substrate,
        _ => {
            return done_err(KError::new(KErrorKind::ShapeError(
                "WITH bindings must be a record literal `{Slot = Type, …}`".to_string(),
            )));
        }
    };
    // Every pin must name a known slot and hold a type. A slot already fixed — a manifest
    // member, which is also what an earlier WITH's fold left behind — admits only an equal
    // re-pin, which normalizes away (never folded), so `S WITH {Tag = Number}` and
    // `(S WITH {A = Number}) WITH {A = Number}` keep their source's signature identity; an
    // unequal re-pin is a type error.
    // The pins are read by the fold below and never leave the step, so they stage on the step
    // scratch. One binding field contributes at most one pin — an equal re-pin of a manifest
    // member normalizes away and pushes nothing — so the field count is an upper bound, which is
    // what taking the capacity up front needs to rule out a regrow.
    let mut pins: BumpVec<'a, (TypeSymbol, KType)> =
        BumpVec::with_capacity_in(bindings.len(), ctx.scratch);
    for (symbol, value) in bindings.fields() {
        // A pin arrives as a raw record-field symbol, which carries no evidence of its token
        // class. The schema's member maps are keyed by classified `TypeSymbol`s and admit a
        // probe by bare symbol bits, so a hit hands back the classification the declaration
        // witnessed — no text resolve, no re-derivation. A pin that is not a Type token was
        // never declared as a member, so it misses both maps and falls to the no-such-slot
        // error, the same disposition an unknown Type name gets. Only that error path — and
        // the two below it — resolves the pin's text.
        let manifest = schema.manifest_members.get_key_value(&symbol);
        let slot = match manifest.or_else(|| schema.abstract_members.get_key_value(&symbol)) {
            Some((slot, _)) => *slot,
            None => {
                return done_err(KError::new(KErrorKind::ShapeError(format!(
                    "{} has no abstract type slot `{}`",
                    sig_handle.display_name(ctx.registries),
                    display_label(symbol, ctx.registries),
                ))));
            }
        };
        let pin_type = match value {
            Held::Type(kt) => *kt,
            Held::Object(other) => {
                return done_err(KError::new(KErrorKind::ShapeError(format!(
                    "WITH binding `{}` value must be a type, got `{}`",
                    display_label(symbol, ctx.registries),
                    other.ktype().display_name(ctx.registries),
                ))));
            }
            Held::UnresolvedType(ti) => {
                return done_err(KError::new(KErrorKind::UnboundName(render_label(
                    ti.symbol(),
                    ctx.registries,
                ))));
            }
            // A pin value arrives as a bound cell from the record's own slots, never as a raw
            // name part.
            Held::Identifier(_) => unreachable!("a WITH pin value is never a captured identifier"),
        };
        match manifest {
            // An equal re-pin of a fixed member normalizes away: not an error, and not a pin.
            Some((_, fixed)) if pin_type == *fixed => {}
            Some((_, fixed)) => {
                return done_err(KError::new(KErrorKind::ShapeError(format!(
                    "`{}.{}` is a manifest type member fixed to `{}`; \
                     WITH cannot re-pin it to `{}`",
                    sig_handle.display_name(ctx.registries),
                    display_label(symbol, ctx.registries),
                    fixed.display_name(ctx.registries),
                    pin_type.display_name(ctx.registries),
                ))));
            }
            None => pins.push((slot, pin_type)),
        }
    }

    let folded = schema.fold_pins(&pins, ctx.types());
    Action::done(Ok(ctx.ctx.type_carried(ctx.types().signature(folded))))
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::lookup_type;
    use crate::builtins::test_support::{TestRun, type_name, value_name};
    use crate::machine::model::{KType, TypeNode};
    use crate::machine::{program_storage, run_root_storage};

    /// `WITH` folds the pin into the schema: the abstract member becomes a manifest one fixed to
    /// the pinned type, so the specialized interface is a fully concrete schema, distinct from
    /// the bare (still-abstract) source.
    #[test]
    fn with_one_slot_folds_the_pin_into_the_schema() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        test_run.run("SIG Ordered = ((TYPE Carrier) (VAL compare :Number))");
        let bare = lookup_type(scope, "Ordered").expect("Ordered binds");
        let result = test_run.run_one_type(test_run.parse_one("Ordered WITH {Carrier = Number}"));
        assert_ne!(result, bare, "a pin refines away from the bare signature");
        match test_run.types().node(result) {
            TypeNode::Signature { schema, .. } => {
                assert!(
                    schema.abstract_members.is_empty(),
                    "the pinned member is no longer abstract",
                );
                assert_eq!(
                    schema
                        .manifest_members
                        .get(&type_name("Carrier", test_run.registries())),
                    Some(&KType::NUMBER),
                );
            }
            _ => panic!("expected Signature type, got {result:?}"),
        }
    }

    /// Folding substitutes slot references to the pinned member: `compare :Carrier` becomes
    /// `compare: Number` under `{Carrier = Number}`, so the specialized interface interns the
    /// same content as a SIG declaring the member manifest outright.
    #[test]
    fn with_fold_substitutes_slot_references_and_unifies_with_concrete_sig() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        test_run.run(
            "SIG Ordered = ((TYPE Carrier) (VAL compare :Carrier))\n\
             SIG IntOrdered = ((LET Carrier = Number) (VAL compare :Carrier))",
        );
        let pinned = test_run.run_one_type(test_run.parse_one("Ordered WITH {Carrier = Number}"));
        match test_run.types().node(pinned) {
            TypeNode::Signature { schema, .. } => {
                assert_eq!(
                    schema
                        .value_slots
                        .get(&value_name("compare", test_run.registries())),
                    Some(&KType::NUMBER),
                    "the slot's reference to Carrier substitutes to the pinned type",
                );
            }
            _ => panic!("expected Signature type, got {pinned:?}"),
        }
        let concrete = lookup_type(scope, "IntOrdered").expect("IntOrdered binds");
        assert_eq!(
            pinned, concrete,
            "a pinned interface and the equivalent concrete declaration are one type",
        );
    }

    /// Pin-set identity is order-independent: both members land as manifest schema entries, so
    /// either record-literal order interns the same specialized type.
    #[test]
    fn with_two_slots_fold_order_independently() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("SIG OrderedSet = ((TYPE Elt) (TYPE Ord) (VAL tag :Number))");
        let result =
            test_run.run_one_type(test_run.parse_one("OrderedSet WITH {Ord = Str, Elt = Number}"));
        match test_run.types().node(result) {
            TypeNode::Signature { schema, .. } => {
                assert_eq!(
                    schema
                        .manifest_members
                        .get(&type_name("Elt", test_run.registries())),
                    Some(&KType::NUMBER),
                );
                assert_eq!(
                    schema
                        .manifest_members
                        .get(&type_name("Ord", test_run.registries())),
                    Some(&KType::STR),
                );
            }
            _ => panic!("expected Signature type, got {result:?}"),
        }
        let literal_order =
            test_run.run_one_type(test_run.parse_one("OrderedSet WITH {Elt = Number, Ord = Str}"));
        assert_eq!(
            result, literal_order,
            "either literal order interns the same specialized type",
        );
    }

    /// Pins accumulate across chained WITH: each chaining order carries both pins and interns
    /// the same type as the one-shot form.
    #[test]
    fn with_pins_accumulate_across_chained_with() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "SIG OrderedSet = ((TYPE Elt) (TYPE Ord) (VAL tag :Number))\n\
             LET ByElt = (OrderedSet WITH {Elt = Number})\n\
             LET ByOrd = (OrderedSet WITH {Ord = Str})",
        );
        let both =
            test_run.run_one_type(test_run.parse_one("OrderedSet WITH {Elt = Number, Ord = Str}"));
        let elt_then_ord = test_run.run_one_type(test_run.parse_one("ByElt WITH {Ord = Str}"));
        let ord_then_elt = test_run.run_one_type(test_run.parse_one("ByOrd WITH {Elt = Number}"));
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
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "SIG OrderedSet = ((TYPE Elt) (TYPE Ord) (VAL tag :Number))\n\
             LET ByElt = (OrderedSet WITH {Elt = Number})",
        );
        let pinned = test_run.run_one_type(test_run.parse_one("OrderedSet WITH {Elt = Number}"));
        let repinned = test_run.run_one_type(test_run.parse_one("ByElt WITH {Elt = Number}"));
        assert_eq!(
            repinned, pinned,
            "an equal re-pin keeps the source identity"
        );
        let err = test_run.run_one_err(test_run.parse_one("ByElt WITH {Elt = Str}"));
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
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "MODULE int_ord = ((LET Carrier = Number) (LET compare = 0))\n\
             SIG Ordered = ((TYPE Carrier) (VAL compare :Number))\n\
             SIG Set = ((TYPE Elt) (VAL insert :Number))\n\
             LET elem = (int_ord :| Ordered)",
        );
        let result = test_run.run_one_type(test_run.parse_one("Set WITH {Elt = elem.Carrier}"));
        let registries = test_run.registries();
        let types = &registries.types;
        match types.node(result) {
            TypeNode::Signature { schema, .. } => {
                let elt = schema
                    .manifest_members
                    .get(&type_name("Elt", registries))
                    .expect("Elt folds to a manifest member");
                match types.node(*elt) {
                    TypeNode::AbstractType { name, .. } => {
                        assert_eq!(name, type_name("Carrier", registries))
                    }
                    _ => panic!("expected Elt = AbstractType(Carrier), got {elt:?}"),
                }
            }
            _ => panic!("expected Signature type, got {result:?}"),
        }
    }

    #[test]
    fn with_rejects_unknown_slot() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        test_run.run("SIG Ordered = ((TYPE Carrier) (VAL compare :Number))");
        let id = test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(
                scope.brand(),
                test_run.parse_one("Ordered WITH {Bogus = Number}"),
            ),
            scope,
        );
        let edge = test_run.runtime.install_edge_for_test(id, scope);
        test_run
            .runtime
            .execute()
            .expect("execute does not surface per-slot errors");
        match test_run.runtime.edge_result_error(edge) {
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
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        test_run.run("SIG Tagged = ((LET Tag = Number) (VAL value :Number))");
        let bare = lookup_type(scope, "Tagged").expect("Tagged must bind a Signature KType");
        let result = test_run.run_one_type(test_run.parse_one("Tagged WITH {Tag = Number}"));
        assert_eq!(
            result, bare,
            "an equal manifest pin must preserve signature identity"
        );
    }

    /// A pin unequal to a manifest member's fixed type is a manifest-fixity error.
    #[test]
    fn with_rejects_unequal_manifest_pin() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        test_run.run("SIG Tagged = ((LET Tag = Number) (VAL value :Number))");
        let id = test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(
                scope.brand(),
                test_run.parse_one("Tagged WITH {Tag = Str}"),
            ),
            scope,
        );
        let edge = test_run.runtime.install_edge_for_test(id, scope);
        test_run
            .runtime
            .execute()
            .expect("execute does not surface per-slot errors");
        match test_run.runtime.edge_result_error(edge) {
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
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("SIG Mixed = ((TYPE Elt) (LET Tag = Number) (VAL value :Number))");
        let result =
            test_run.run_one_type(test_run.parse_one("Mixed WITH {Elt = Str, Tag = Number}"));
        match test_run.types().node(result) {
            TypeNode::Signature { schema, .. } => {
                assert!(schema.abstract_members.is_empty());
                assert_eq!(
                    schema
                        .manifest_members
                        .get(&type_name("Elt", test_run.registries())),
                    Some(&KType::STR),
                );
                assert_eq!(
                    schema
                        .manifest_members
                        .get(&type_name("Tag", test_run.registries())),
                    Some(&KType::NUMBER),
                );
            }
            _ => panic!("expected Signature type, got {result:?}"),
        }
    }

    #[test]
    fn with_rejects_lowercase_slot_name() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        test_run.run("SIG Ordered = ((TYPE Carrier) (VAL compare :Number))");
        let id = test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(
                scope.brand(),
                test_run.parse_one("Ordered WITH {type = Number}"),
            ),
            scope,
        );
        let edge = test_run.runtime.install_edge_for_test(id, scope);
        test_run
            .runtime
            .execute()
            .expect("execute does not surface per-slot errors");
        match test_run.runtime.edge_result_error(edge) {
            Err(e) => assert!(
                format!("{e}").contains("no abstract type slot"),
                "expected lowercase-slot rejection, got {e}",
            ),
            Ok(()) => panic!("WITH with lowercase slot must err"),
        }
    }
}
