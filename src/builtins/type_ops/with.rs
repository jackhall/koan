//! `<sig> WITH {<Slot> = <Type>, …}` — infix signature specialization. Pins a subset of
//! `sig`'s abstract-type slots, each to the type bound in the record literal, yielding a
//! `KType::Signature { sig, pinned_slots }`. The infix signature-specialization builtin.
//!
//! The `{Slot = Type}` record literal eager-evaluates to a `KObject::Record` whose field
//! values are resolved `KTypeValue`s — a dotted `Er.Type` value sub-dispatches in value
//! context for free — so the body reads `(name, KTypeValue)` entries directly: no lazy
//! binding slot, no `Combine`.

use std::collections::HashSet;

use crate::machine::model::{Held, KObject, KType};
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, SchedulerHandle, Scope};

use crate::builtins::ascribe::abstract_type_names_of;
use crate::builtins::err;

pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a, 'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let s = match bundle.require_signature("sig") {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    let fields = match bundle.get("bindings") {
        Some(KObject::Record(fields, _types)) => fields,
        _ => {
            return err(KError::new(KErrorKind::ShapeError(
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
            return err(KError::new(KErrorKind::ShapeError(format!(
                "{} has no abstract type slot `{name}`",
                s.path,
            ))));
        }
        match value {
            Held::Type(kt) => pinned.push((name.clone(), kt.clone())),
            Held::Object(other) => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "WITH binding `{name}` value must be a type, got `{}`",
                    other.ktype().name(),
                ))));
            }
        }
    }
    BodyResult::ktype(scope.arena.alloc_ktype(KType::Signature {
        sig: s,
        pinned_slots: pinned,
    }))
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one_type, run_root_silent};
    use crate::machine::execute::Scheduler;
    use crate::machine::model::KType;
    use crate::machine::RuntimeArena;

    #[test]
    fn with_one_slot_pins_the_named_slot() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))",
        );
        let sig_id = match scope.resolve_type("OrderedSig") {
            Some(KType::Signature { sig, .. }) => sig.sig_id(),
            _ => panic!("OrderedSig must bind a Signature KType"),
        };
        let result = run_one_type(scope, parse_one("OrderedSig WITH {Type = Number}"));
        match result {
            KType::Signature { sig, pinned_slots } => {
                assert_eq!(sig.sig_id(), sig_id);
                assert_eq!(sig.path, "OrderedSig");
                assert_eq!(pinned_slots.len(), 1);
                assert_eq!(pinned_slots[0].0, "Type");
                assert_eq!(pinned_slots[0].1, KType::Number);
            }
            other => panic!("expected Signature type, got {other:?}"),
        }
    }

    /// Pins land in record-literal order — `pinned_slots` is an ordered `Vec`.
    #[test]
    fn with_two_slots_preserve_order() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
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

    /// A dotted `Elem.Type` pin value sub-dispatches in value context to the abstract
    /// `Type` and surfaces in `pinned_slots` — a dotted pin value the keyworded record-literal
    /// handler could not take (was `#[ignore]`d there).
    #[test]
    fn with_inner_module_attr_path_pins_abstract_type() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "MODULE IntOrd = ((LET Type = Number) (LET compare = 0))\n\
             SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
             SIG SetSig = ((LET Elt = Number) (VAL insert :Number))\n\
             LET Elem = (IntOrd :| OrderedSig)",
        );
        let result = run_one_type(scope, parse_one("SetSig WITH {Elt = Elem.Type}"));
        match result {
            KType::Signature { pinned_slots, .. } => {
                assert_eq!(pinned_slots.len(), 1);
                assert_eq!(pinned_slots[0].0, "Elt");
                match &pinned_slots[0].1 {
                    KType::AbstractType { name, .. } => assert_eq!(name, "Type"),
                    other => panic!("expected pinned Elt = AbstractType(Type), got {:?}", other),
                }
            }
            other => panic!("expected Signature type, got {other:?}"),
        }
    }

    #[test]
    fn with_rejects_unknown_slot() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))",
        );
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("OrderedSig WITH {Bogus = Number}"), scope);
        sched
            .execute()
            .expect("execute does not surface per-slot errors");
        match sched.read_result(id) {
            Err(e) => assert!(
                format!("{e}").contains("no abstract type slot"),
                "expected unknown-slot rejection, got {e}",
            ),
            Ok(_) => panic!("WITH on unknown slot must err"),
        }
    }

    #[test]
    fn with_rejects_lowercase_slot_name() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))",
        );
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("OrderedSig WITH {type = Number}"), scope);
        sched
            .execute()
            .expect("execute does not surface per-slot errors");
        match sched.read_result(id) {
            Err(e) => assert!(
                format!("{e}").contains("no abstract type slot"),
                "expected lowercase-slot rejection, got {e}",
            ),
            Ok(_) => panic!("WITH with lowercase slot must err"),
        }
    }
}
