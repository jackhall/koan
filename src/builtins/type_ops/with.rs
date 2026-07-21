//! `<sig> WITH {<Slot> = <Type>, …}` — infix signature specialization. Pins a subset of
//! `sig`'s abstract-type slots, each to the type bound in the record literal, interning a
//! `Signature` node carrying the same schema plus the `pinned_slots`. A pin naming a manifest
//! member (its type already fixed) is not an abstract slot: a pin equal to the fixed type
//! normalizes away (leaving signature identity unchanged), and an unequal one is a type error.
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
    // Validation only: every pin must name a known slot and hold a type. A pin equal to a
    // manifest member's fixed type is normalized away (added to `dropped`, never recorded), so
    // `S WITH {Tag = Number}` keeps `S`'s signature identity; an unequal manifest pin is a type
    // error. `dropped` names the pins the composed `Signature` skips.
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

    // The pins and the signature content are owned data, so the specialized `Signature` composes
    // directly and allocates into this step's own region through the single type door.
    let pinned: Vec<(String, KType)> = fields
        .iter()
        .filter(|(name, _)| !dropped.contains(name.as_str()))
        .map(|(name, value)| match value {
            Held::Type(kt) => (name.clone(), *kt),
            Held::Object(_) | Held::UnresolvedType(_) => {
                unreachable!("validated above: every pin value is a type")
            }
        })
        .collect();
    Action::Done(Ok(ctx
        .ctx
        .type_carried(ctx.types.signature(schema, pinned))))
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, TestRun};
    use crate::machine::model::{KType, TypeNode};
    use crate::machine::run_root_storage;

    /// `WITH` pins the named slot of *this* signature: the pinned result carries the same schema
    /// (a signature carries no `sig_id`/`path` since ruling 12, so schema-content identity is what
    /// witnesses "the same signature").
    #[test]
    fn with_one_slot_pins_the_named_slot() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        let scope = test_run.scope;
        test_run.run("SIG Ordered = ((TYPE Carrier) (VAL compare :Number))");
        let bare_schema_digest = {
            let types = test_run.types();
            let bare = scope.resolve_type("Ordered").expect("Ordered binds");
            match types.node(bare) {
                TypeNode::Signature { schema_digest, .. } => schema_digest,
                _ => panic!("Ordered must bind a Signature KType"),
            }
        };
        let result = test_run.run_one_type(parse_one("Ordered WITH {Carrier = Number}"));
        match test_run.types().node(result) {
            TypeNode::Signature {
                schema_digest,
                pinned_slots,
                ..
            } => {
                assert_eq!(
                    schema_digest, bare_schema_digest,
                    "the pin is over the same signature schema",
                );
                assert_eq!(pinned_slots.len(), 1);
                assert_eq!(pinned_slots[0].0, "Carrier");
                assert_eq!(pinned_slots[0].1, KType::NUMBER);
            }
            _ => panic!("expected Signature type, got {result:?}"),
        }
    }

    /// Pins land in record-literal order — `pinned_slots` is an ordered `Vec`.
    #[test]
    fn with_two_slots_preserve_order() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("SIG OrderedSet = ((TYPE Elt) (TYPE Ord) (VAL tag :Number))");
        let result = test_run.run_one_type(parse_one("OrderedSet WITH {Elt = Number, Ord = Str}"));
        match test_run.types().node(result) {
            TypeNode::Signature { pinned_slots, .. } => {
                assert_eq!(pinned_slots.len(), 2);
                assert_eq!(pinned_slots[0].0, "Elt");
                assert_eq!(pinned_slots[0].1, KType::NUMBER);
                assert_eq!(pinned_slots[1].0, "Ord");
                assert_eq!(pinned_slots[1].1, KType::STR);
            }
            _ => panic!("expected Signature type, got {result:?}"),
        }
    }

    /// A dotted `elem.Carrier` pin value sub-dispatches in value context to the abstract
    /// `Carrier` and surfaces in `pinned_slots` — a dotted pin value the keyworded record-literal
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
            TypeNode::Signature { pinned_slots, .. } => {
                assert_eq!(pinned_slots.len(), 1);
                assert_eq!(pinned_slots[0].0, "Elt");
                match types.node(pinned_slots[0].1) {
                    TypeNode::AbstractType { name, .. } => assert_eq!(name, "Carrier"),
                    _ => panic!(
                        "expected pinned Elt = AbstractType(Carrier), got {:?}",
                        pinned_slots[0].1
                    ),
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

    /// A pin equal to a manifest member's fixed type normalizes away: `pinned_slots` stays
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
        match test_run.types().node(result) {
            TypeNode::Signature { pinned_slots, .. } => {
                assert!(
                    pinned_slots.is_empty(),
                    "equal manifest pin must not record a pinned slot, got {pinned_slots:?}"
                );
            }
            _ => panic!("expected Signature type, got {result:?}"),
        }
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

    /// A mixed record pins only the abstract slot; the equal manifest pin normalizes away.
    #[test]
    fn with_mixed_record_records_only_abstract_pin() {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run("SIG Mixed = ((TYPE Elt) (LET Tag = Number) (VAL value :Number))");
        let result = test_run.run_one_type(parse_one("Mixed WITH {Elt = Str, Tag = Number}"));
        match test_run.types().node(result) {
            TypeNode::Signature { pinned_slots, .. } => {
                assert_eq!(
                    pinned_slots.len(),
                    1,
                    "only the abstract Elt pin is recorded, got {pinned_slots:?}"
                );
                assert_eq!(pinned_slots[0].0, "Elt");
                assert_eq!(pinned_slots[0].1, KType::STR);
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
