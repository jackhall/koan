//! The per-step door's facts: over a cell with no layers of its own, [`Facts`] answers every
//! reader as the load's facts it rests on do.

use crate::scope::{Facts, Slot};

use super::statics::loaded;

/// A program reaching most kinds of fact: declared and registered types, a quantified callable and
/// its instance, a module, a view, a quote, a keyworded call and a call by name.
const SOURCE: &str = "\
NEWTYPE Meters = Number
LET twice = FN EXPR #(TWICE x :Number) -> Number = #(x)
EXPR FOR ALL #[Elt] #(SAME x :Elt) -> Elt = #(x)
LET seven = (SAME 7)
MODULE m = ((LET zero = 0) (NEWTYPE Dist = Number))
LET q = #(TWICE 3)
PRINT (TWICE seven)";

#[test]
fn facts_over_no_layers_answer_as_the_load_does() {
    loaded(SOURCE, |program| {
        let load = program.facts();
        let facts = Facts::of(load);
        let types = program.tiers().resting();
        let same = |what: &str, left: String, right: String| {
            assert_eq!(left, right, "{what} reads alike through the door");
        };
        for shape in load.shapes().bodies() {
            let shape = *shape;
            // A reader whose fact names a type reads it over the chain: `typed!` hands it one.
            macro_rules! alike {
                ($reader:ident ( $($argument:expr),* )) => {
                    same(
                        stringify!($reader),
                        format!("{:?}", load.$reader(shape, $($argument),*)),
                        format!("{:?}", facts.$reader(shape, $($argument),*)),
                    )
                };
            }
            macro_rules! typed {
                ($reader:ident ( $($argument:expr),* )) => {
                    same(
                        stringify!($reader),
                        format!("{:?}", load.$reader(&types, shape, $($argument),*)),
                        format!("{:?}", facts.$reader(&types, shape, $($argument),*)),
                    )
                };
            }
            typed!(callable_type());
            typed!(group_levels());
            typed!(born_instance());
            alike!(homes());
            alike!(type_captures());
            alike!(capture_count());
            alike!(contract());
            alike!(class());
            alike!(caches());
            alike!(digested_count());
            typed!(overlap());
            typed!(typing_refusal());
            for slot in (0..shape.slots()).map(|slot| Slot(slot as u32)) {
                typed!(declared_type(slot));
                typed!(registered_type(slot));
                typed!(binder_type(slot));
                typed!(content(slot));
                alike!(digested_index(slot));
            }
            for site in shape.sites().all() {
                typed!(typed_expression(site));
                typed!(value_type(site));
                typed!(use_statics(site));
                alike!(settled(site));
                typed!(instance_at(site));
                typed!(selection(site));
                typed!(named_contributions(site));
                alike!(mint_at(site));
                alike!(mints_at(site));
                typed!(view_tree(site));
                alike!(path_read(site));
            }
            for index in 0..shape.body().len() {
                typed!(statement_type(index));
            }
        }
    });
}
