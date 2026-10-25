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
        let same = |what: &str, left: String, right: String| {
            assert_eq!(left, right, "{what} reads alike through the door");
        };
        for shape in load.shapes().bodies() {
            let shape = *shape;
            macro_rules! alike {
                ($reader:ident ( $($argument:expr),* )) => {
                    same(
                        stringify!($reader),
                        format!("{:?}", load.$reader(shape, $($argument),*)),
                        format!("{:?}", facts.$reader(shape, $($argument),*)),
                    )
                };
            }
            alike!(callable_type());
            alike!(group_levels());
            alike!(born_instance());
            alike!(homes());
            alike!(type_captures());
            alike!(capture_count());
            alike!(contract());
            alike!(class());
            alike!(caches());
            alike!(digested_count());
            alike!(overlap());
            alike!(typing_refusal());
            for slot in (0..shape.slots()).map(|slot| Slot(slot as u32)) {
                alike!(declared_type(slot));
                alike!(registered_type(slot));
                alike!(binder_type(slot));
                alike!(content(slot));
                alike!(digested_index(slot));
            }
            for site in shape.sites().all() {
                alike!(typed_expression(site));
                alike!(value_type(site));
                alike!(use_statics(site));
                alike!(settled(site));
                alike!(instance_at(site));
                alike!(selection(site));
                alike!(named_contributions(site));
                alike!(mint_at(site));
                alike!(mints_at(site));
                alike!(view_tree(site));
                alike!(path_read(site));
            }
            for index in 0..shape.body().len() {
                alike!(statement_type(index));
            }
        }
    });
}
