//! `<sig> WITH {<Slot> = <Type>, …}` — infix signature specialization. Pins a subset of
//! `sig`'s abstract-type slots, each to the type bound in the record literal, yielding a
//! `KType::Signature { sig, pinned_slots }`. The infix signature-specialization builtin.
//!
//! The `{Slot = Type}` record literal eager-evaluates to a `KObject::Record` whose field
//! values are resolved `Held::Type`s — a dotted `Er.Type` value sub-dispatches in value
//! context for free — so the body reads `(name, Held::Type)` entries directly: no lazy
//! binding slot, no `AwaitDeps`.

use std::collections::HashSet;

use crate::machine::core::KoanStepContextExt;
use crate::machine::model::{Held, KObject, KType};
use crate::machine::{KError, KErrorKind};

use crate::builtins::ascribe::abstract_type_names_of;

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
    // A binding must name one of the SIG's abstract type slots — a width-subset check
    // against the slot set. Slot names are capitalized, so a lowercase / unknown key
    // simply isn't in the set; no separate name-shape check is needed.
    let known_slots: HashSet<String> = abstract_type_names_of(s.decl_scope()).into_iter().collect();
    let mut pinned: Vec<(String, KType<'a>)> = Vec::with_capacity(fields.len());
    for (name, value) in fields.iter() {
        if !known_slots.contains(name) {
            return done_err(KError::new(KErrorKind::ShapeError(format!(
                "{} has no abstract type slot `{name}`",
                s.path,
            ))));
        }
        match value {
            Held::Type(kt) => pinned.push((name.clone(), kt.clone())),
            Held::Object(other) => {
                return done_err(KError::new(KErrorKind::ShapeError(format!(
                    "WITH binding `{name}` value must be a type, got `{}`",
                    other.ktype().name(),
                ))));
            }
        }
    }
    Action::Done(Ok(ctx.ctx.alloc_type(KType::Signature {
        sig: s,
        pinned_slots: pinned,
    })))
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one_type, run_root_silent};
    use crate::machine::core::FrameStorage;
    use crate::machine::execute::KoanRuntime;
    use crate::machine::model::KType;

    #[test]
    fn with_one_slot_pins_the_named_slot() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(
            scope,
            "SIG OrderedSig = ((LET Carrier = Number) (VAL compare :Number))",
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
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(
            scope,
            "SIG Set = ((LET Elt = Number) (LET Ord = Number) (VAL tag :Number))",
        );
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
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(
            scope,
            "MODULE IntOrd = ((LET Carrier = Number) (LET compare = 0))\n\
             SIG OrderedSig = ((LET Carrier = Number) (VAL compare :Number))\n\
             SIG SetSig = ((LET Elt = Number) (VAL insert :Number))\n\
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
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(
            scope,
            "SIG OrderedSig = ((LET Carrier = Number) (VAL compare :Number))",
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

    #[test]
    fn with_rejects_lowercase_slot_name() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(
            scope,
            "SIG OrderedSig = ((LET Carrier = Number) (VAL compare :Number))",
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
