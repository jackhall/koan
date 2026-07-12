//! `<sig> WITH {<Slot> = <Type>, …}` — infix signature specialization. Pins a subset of
//! `sig`'s abstract-type slots, each to the type bound in the record literal, yielding a
//! `KType::Signature { sig, pinned_slots }`. A pin naming a manifest member (its type already
//! fixed) is not an abstract slot: a pin equal to the fixed type normalizes away (leaving
//! signature identity unchanged), and an unequal one is a type error.
//!
//! The `{Slot = Type}` record literal eager-evaluates to a `KObject::Record` whose field
//! values are resolved `Held::Type`s — a dotted `Er.Type` value sub-dispatches in value
//! context for free — so the body reads `(name, Held::Type)` entries directly: no lazy
//! binding slot, no `AwaitDeps`.

use std::collections::HashSet;

use crate::machine::model::{Carried, Held, KObject, KType};
use crate::machine::{KError, KErrorKind};

use crate::machine::model::types::{abstract_members_of, manifest_type_members_of};

/// `<sig> WITH {<Slot> = <Type>, …}`: reads the `sig` type cell and the eager-evaluated `bindings`
/// record from `BodyCtx::args`, validates each pin against the SIG's abstract type slots, and
/// returns the specialized `KType::Signature` as a `Carried::Type`.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{arg_held, arg_object, arg_type, Action};

    let done_err = |e: KError| Action::Done(Err(e));
    let s = match arg_type(ctx.args, "sig") {
        Some(KType::Signature { sig, .. }) => *sig,
        other => {
            let got = match (other, arg_held(ctx.args, "sig")) {
                (Some(kt), _) => kt.name(),
                (None, Some(Held::Object(object))) => object.ktype().name(),
                (None, _) => {
                    return done_err(KError::new(KErrorKind::MissingArg("sig".to_string())))
                }
            };
            return done_err(KError::new(KErrorKind::TypeMismatch {
                arg: "sig".to_string(),
                expected: "Signature".to_string(),
                got,
            }));
        }
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
    let abstract_slots: HashSet<String> = abstract_members_of(s.decl_scope()).into_iter().collect();
    let manifest_members: std::collections::HashMap<String, KType> =
        manifest_type_members_of(s.decl_scope())
            .into_iter()
            .collect();
    // Validation only: every pin must name a known slot and hold a type. A pin equal to a
    // manifest member's fixed type is normalized away (added to `dropped`, never recorded), so
    // `S WITH {Tag = Number}` keeps `S`'s signature identity; an unequal manifest pin is a type
    // error. The composed `Signature` is built inside the fold closure from the crossed views,
    // never from an ambient `pinned` vec — the compiler rejects smuggling an `'a` `KType` into
    // the fresh brand — so `dropped` (a `String`-only set) is threaded in for the fold to skip.
    let mut dropped: HashSet<String> = HashSet::new();
    for (name, value) in fields.iter() {
        let is_abstract = abstract_slots.contains(name);
        let manifest = manifest_members.get(name);
        if !is_abstract && manifest.is_none() {
            return done_err(KError::new(KErrorKind::ShapeError(format!(
                "{} has no abstract type slot `{name}`",
                s.path,
            ))));
        }
        let pin_type = match value {
            Held::Type(kt) => kt,
            Held::Object(other) => {
                return done_err(KError::new(KErrorKind::ShapeError(format!(
                    "WITH binding `{name}` value must be a type, got `{}`",
                    other.ktype().name(),
                ))));
            }
        };
        if let Some(fixed) = manifest {
            if pin_type == fixed {
                dropped.insert(name.clone());
            } else {
                return done_err(KError::new(KErrorKind::ShapeError(format!(
                    "`{}.{name}` is a manifest type member fixed to `{}`; \
                     WITH cannot re-pin it to `{}`",
                    s.path,
                    fixed.render(),
                    pin_type.render(),
                ))));
            }
        }
    }

    // A `Signature` type always borrows its decl scope's region (`to_static` declines it), so the
    // sig operand must cross as a carrier.
    let sig_carrier = match ctx.arg_carrier("sig") {
        Some(carrier) => carrier.duplicate(),
        None => {
            return done_err(KError::new(KErrorKind::ShapeError(
                "WITH signature operand reaches a region but arrived without a carrier".to_string(),
            )))
        }
    };
    match ctx.arg_carrier("bindings") {
        // Both operands cross the build brand as dep views: the sig arm supplies the signature
        // pointer, the pins are cloned out of the record view at the brand, and both carriers'
        // reach unions into the result's witness.
        Some(bindings) => {
            let bindings_carrier = bindings.duplicate();
            let sealed = ctx.ctx.alloc_carried_with(
                &[&sig_carrier, &bindings_carrier],
                move |brand, views| {
                    let sig = match views[0] {
                        Carried::Type(KType::Signature { sig, .. }) => sig,
                        _ => unreachable!("validated above: the sig arg is a Signature type"),
                    };
                    let pinned: Vec<(String, KType)> = match views[1] {
                        Carried::Object(KObject::Record(record, _)) => record
                            .iter()
                            .filter(|(name, _)| !dropped.contains(name.as_str()))
                            .map(|(name, value)| match value {
                                Held::Type(kt) => (name.clone(), kt.clone()),
                                Held::Object(_) => {
                                    unreachable!("validated above: every pin value is a type")
                                }
                            })
                            .collect(),
                        _ => unreachable!("validated above: bindings is a record"),
                    };
                    Carried::Type(brand.alloc_ktype_folded(KType::Signature {
                        sig,
                        pinned_slots: pinned,
                    }))
                },
            );
            Action::Done(Ok(sealed))
        }
        // Carrier-less bindings: every pin must be region-free; rebuild the pin list at the brand
        // from its `'static` plan. A pin type that reaches a region without a carrier errors loudly.
        None => {
            let plan: Option<Vec<(String, KType<'static>)>> = fields
                .iter()
                .filter(|(name, _)| !dropped.contains(name.as_str()))
                .map(|(name, value)| match value {
                    Held::Type(kt) => kt.to_static().map(|owned| (name.clone(), owned)),
                    Held::Object(_) => unreachable!("validated above: every pin value is a type"),
                })
                .collect();
            match plan {
                Some(plan) => {
                    let sealed = ctx.ctx.alloc_carried_with(&[&sig_carrier], move |brand, views| {
                        let sig = match views[0] {
                            Carried::Type(KType::Signature { sig, .. }) => sig,
                            _ => unreachable!("validated above: the sig arg is a Signature type"),
                        };
                        let pinned: Vec<(String, KType)> = plan
                            .into_iter()
                            .map(|(name, kt)| (name, brand.alloc_ktype(kt).clone()))
                            .collect();
                        Carried::Type(brand.alloc_ktype_folded(KType::Signature {
                            sig,
                            pinned_slots: pinned,
                        }))
                    });
                    Action::Done(Ok(sealed))
                }
                None => done_err(KError::new(KErrorKind::ShapeError(
                    "WITH pin type reaches a region but the bindings record arrived without a carrier"
                        .to_string(),
                ))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one_type, run_root_silent};
    use crate::machine::core::run_root_storage;
    use crate::machine::execute::KoanRuntime;
    use crate::machine::model::KType;

    #[test]
    fn with_one_slot_pins_the_named_slot() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(
            scope,
            "SIG OrderedSig = ((TYPE Carrier) (VAL compare :Number))",
        );
        let sig_id = match scope.resolve_type("OrderedSig") {
            Some(KType::Signature { sig, .. }) => sig.sig_id(),
            _ => panic!("OrderedSig must bind a Signature KType"),
        };
        let result = run_one_type(scope, parse_one("OrderedSig WITH {Carrier = Number}"));
        match result {
            KType::Signature { sig, pinned_slots } => {
                assert_eq!(sig.sig_id(), sig_id);
                assert_eq!(sig.path, "OrderedSig");
                assert_eq!(pinned_slots.len(), 1);
                assert_eq!(pinned_slots[0].0, "Carrier");
                assert_eq!(pinned_slots[0].1, KType::Number);
            }
            other => panic!("expected Signature type, got {other:?}"),
        }
    }

    /// Pins land in record-literal order — `pinned_slots` is an ordered `Vec`.
    #[test]
    fn with_two_slots_preserve_order() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "SIG Set = ((TYPE Elt) (TYPE Ord) (VAL tag :Number))");
        let result = run_one_type(scope, parse_one("Set WITH {Elt = Number, Ord = Str}"));
        match result {
            KType::Signature { pinned_slots, .. } => {
                assert_eq!(pinned_slots.len(), 2);
                assert_eq!(pinned_slots[0].0, "Elt");
                assert_eq!(pinned_slots[0].1, KType::Number);
                assert_eq!(pinned_slots[1].0, "Ord");
                assert_eq!(pinned_slots[1].1, KType::Str);
            }
            other => panic!("expected Signature type, got {other:?}"),
        }
    }

    /// A dotted `Elem.Carrier` pin value sub-dispatches in value context to the abstract
    /// `Carrier` and surfaces in `pinned_slots` — a dotted pin value the keyworded record-literal
    /// handler could not take (was `#[ignore]`d there).
    #[test]
    fn with_inner_module_attr_path_pins_abstract_type() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(
            scope,
            "MODULE IntOrd = ((LET Carrier = Number) (LET compare = 0))\n\
             SIG OrderedSig = ((TYPE Carrier) (VAL compare :Number))\n\
             SIG SetSig = ((TYPE Elt) (VAL insert :Number))\n\
             LET Elem = (IntOrd :| OrderedSig)",
        );
        let result = run_one_type(scope, parse_one("SetSig WITH {Elt = Elem.Carrier}"));
        match result {
            KType::Signature { pinned_slots, .. } => {
                assert_eq!(pinned_slots.len(), 1);
                assert_eq!(pinned_slots[0].0, "Elt");
                match &pinned_slots[0].1 {
                    KType::AbstractType { name, .. } => assert_eq!(name, "Carrier"),
                    other => panic!(
                        "expected pinned Elt = AbstractType(Carrier), got {:?}",
                        other
                    ),
                }
            }
            other => panic!("expected Signature type, got {other:?}"),
        }
    }

    #[test]
    fn with_rejects_unknown_slot() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(
            scope,
            "SIG OrderedSig = ((TYPE Carrier) (VAL compare :Number))",
        );
        let mut runtime = KoanRuntime::new();
        let id = runtime.dispatch_in_scope(parse_one("OrderedSig WITH {Bogus = Number}"), scope);
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
        let scope = run_root_silent(&region);
        run(
            scope,
            "SIG Tagged = ((LET Tag = Number) (VAL value :Number))",
        );
        let bare = scope
            .resolve_type("Tagged")
            .expect("Tagged must bind a Signature KType");
        let result = run_one_type(scope, parse_one("Tagged WITH {Tag = Number}"));
        match &result {
            KType::Signature { pinned_slots, .. } => {
                assert!(
                    pinned_slots.is_empty(),
                    "equal manifest pin must not record a pinned slot, got {pinned_slots:?}"
                );
            }
            other => panic!("expected Signature type, got {other:?}"),
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
        let scope = run_root_silent(&region);
        run(
            scope,
            "SIG Tagged = ((LET Tag = Number) (VAL value :Number))",
        );
        let mut runtime = KoanRuntime::new();
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
        let scope = run_root_silent(&region);
        run(
            scope,
            "SIG Mixed = ((TYPE Elt) (LET Tag = Number) (VAL value :Number))",
        );
        let result = run_one_type(scope, parse_one("Mixed WITH {Elt = Str, Tag = Number}"));
        match result {
            KType::Signature { pinned_slots, .. } => {
                assert_eq!(
                    pinned_slots.len(),
                    1,
                    "only the abstract Elt pin is recorded, got {pinned_slots:?}"
                );
                assert_eq!(pinned_slots[0].0, "Elt");
                assert_eq!(pinned_slots[0].1, KType::Str);
            }
            other => panic!("expected Signature type, got {other:?}"),
        }
    }

    #[test]
    fn with_rejects_lowercase_slot_name() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(
            scope,
            "SIG OrderedSig = ((TYPE Carrier) (VAL compare :Number))",
        );
        let mut runtime = KoanRuntime::new();
        let id = runtime.dispatch_in_scope(parse_one("OrderedSig WITH {type = Number}"), scope);
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
