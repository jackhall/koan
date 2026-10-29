//! The load's side of a read ([lookup.rs](../lookup.rs)): `(head argument)` where the head's
//! static type is a list, a dict or a string, typed at both ends of that type.
//!
//! The head is read as a container where either end of its static type is one; a variable, a
//! union, `Any` or `Never` is none, so a head typed by a variable bounded by a list types as an
//! unknown head does. Where only the lower end is a container, the upper end's element, key and
//! value are `Any`. The argument's lower end decides the refusals: a list or a string refuses one
//! lying outside `Number | {}`, a record naming a bound outside `Number`, and a record naming
//! neither bound; a dict refuses a key outside `Str | Number | Bool` or outside its key type. The
//! read's upper end is `Result {Ok = O, Error = E}`: an index reads the element (`Str` for a
//! string), a slice the container's own type, either `E = Number`; an argument that may be either
//! reads their union. A lookup reads the value type, `E` the key type where the argument's upper
//! end lies under it and the union of both otherwise. Its lower end is `Never`: the run answers
//! either variant.

use crate::memory::BumpVec;
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{ShapeError, Store};
use crate::symbols::{BinderSymbol, TypeSymbol};
use crate::type_lattice::{
    Interval, KType, Parametric, TypeNode, bound_above, is_subtype_of, lower_end_outside,
};

use super::super::builtins::result_type;
use super::super::lookup::bound;
use super::Bound;

/// The `Result` parameter `name` names, as a construction's argument.
fn variant(name: &str) -> BinderSymbol {
    BinderSymbol::Type(TypeSymbol::classify(name).expect("a Type token"))
}

/// What one end of a read's head type is.
#[derive(Clone, Copy)]
enum Container {
    List { element: Parametric },
    Dict { key: Parametric, value: Parametric },
    Str,
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>>
    super::Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// What `end` is as a read's head, read off its node: `None` for anything but a list, a dict or
    /// `Str`.
    fn container(&self, end: Parametric) -> Option<Container> {
        if end == KType::STR.into() {
            return Some(Container::Str);
        }
        match self.types.node(end) {
            TypeNode::List { element } => Some(Container::List { element }),
            TypeNode::Dict { key, value } => Some(Container::Dict { key, value }),
            _ => None,
        }
    }

    /// The static type of `node`, `(head argument)`, read where `typed`, the head's static type,
    /// is a container at either end; `None` where it is none, which leaves the application a call
    /// by name. Refused where the argument's lower end can never read the head.
    pub(super) fn read(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        (head, typed): (&'graph ExpressionPart<'graph>, Bound),
        argument: &'graph ExpressionPart<'graph>,
    ) -> Result<Option<Interval>, ShapeError<'o>> {
        let Bound::Type(typed) = typed else {
            return Ok(None);
        };
        let shape = match (self.container(typed.upper), self.container(typed.lower)) {
            (Some(upper), _) => upper,
            // A carried type above a list is a list: the upper end's parts are `Any`.
            (None, Some(Container::List { .. })) => Container::List {
                element: KType::ANY.into(),
            },
            (None, Some(Container::Dict { .. })) => Container::Dict {
                key: KType::ANY.into(),
                value: KType::ANY.into(),
            },
            (None, Some(Container::Str)) => Container::Str,
            (None, None) => return Ok(None),
        };
        let payload = self.part(level, argument)?;
        let (types, scratch) = (self.types, self.scratch);
        let outside = |slot: Parametric| lower_end_outside(types, scratch, payload, slot);
        let number: Parametric = KType::NUMBER.into();
        let union = |members: &[Parametric]| types.union_of(scratch, members);
        let bounds = |end: Parametric| match types.node(end) {
            TypeNode::Record { fields } => {
                let named = |name| fields.get(bound(name));
                Some([named("from"), named("to")])
            }
            _ => None,
        };
        let refused = match shape {
            Container::Dict { key, .. } => {
                let scalar = [KType::STR, KType::NUMBER, KType::BOOL].map(Parametric::from);
                outside(union(&scalar)) || outside(key)
            }
            _ => {
                outside(union(&[number, KType::EMPTY_RECORD.into()]))
                    || bounds(payload.lower).is_some_and(|named| {
                        named.iter().all(Option::is_none)
                            || named.iter().flatten().any(|bound| {
                                lower_end_outside(types, scratch, Interval::point(*bound), number)
                            })
                    })
            }
        };
        if refused {
            return Err(ShapeError::ReadNeverSatisfied {
                container: bound_above(types, scratch, typed.upper),
                argument: bound_above(types, scratch, payload.upper),
                at: node.source,
            });
        }
        let (ok, error) = match shape {
            Container::Dict { key, value } => {
                let under = types.concrete(payload.upper).is_some_and(|upper| {
                    types
                        .concrete(key)
                        .is_some_and(|key| is_subtype_of(types, scratch, upper, key))
                });
                (
                    value,
                    if under {
                        key
                    } else {
                        union(&[key, payload.upper])
                    },
                )
            }
            _ => {
                let element = match shape {
                    Container::List { element } => element,
                    _ => KType::STR.into(),
                };
                let head_upper = match self.container(typed.upper) {
                    Some(_) => typed.upper,
                    None => KType::ANY.into(),
                };
                let ok = if payload.upper == number || payload.upper == KType::NEVER.into() {
                    element
                } else if bounds(payload.upper)
                    .is_some_and(|named| named.iter().any(Option::is_some))
                {
                    head_upper
                } else {
                    union(&[element, head_upper])
                };
                (ok, number)
            }
        };
        let mut arguments = BumpVec::with_capacity_in(2, scratch);
        arguments.extend([(variant("Ok"), ok), (variant("Error"), error)]);
        arguments.sort_unstable_by_key(|(name, _)| *name);
        let result = result_type(self.builtins);
        let TypeNode::Union { members } = types.node(result) else {
            unreachable!("the builtin `Result` is a union")
        };
        let mut variants = BumpVec::with_capacity_in(2, scratch);
        variants.extend(
            members
                .iter()
                .map(|variant| types.constructor_apply(scratch, variant, &arguments)),
        );
        let upper = types.union_of(scratch, &variants);
        self.record_composite(
            level,
            self.node_site(level, node),
            [(head, typed), (argument, payload)].into_iter(),
        );
        Ok(Some(Interval {
            lower: KType::NEVER.into(),
            upper,
        }))
    }
}
