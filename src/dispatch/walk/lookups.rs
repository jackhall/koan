//! The load's side of a read ([lookup.rs](../lookup.rs)): `(head argument)` where the head's
//! static type is a list, a dict, a string or code, typed at both ends of that type.
//!
//! The head is read as a container where either end of its static type is one; a union, `Any` or
//! `Never` is none. A lexical variable, or a projection of one, whose bound is a list, a dict, a
//! string or code is read as that container, its element, key and value its projections there
//! (`(ELEMENT OF Ll)`), which name the variable and judge nothing. Any other run-bound name is
//! read by what a binding gives it — a read, a call by name or neither — through one closed rule
//! ([`Rule::Application`](crate::scope::Rule::Application)); a head typed by a signature's head
//! parameter or a path stays unknown: a view's carrier lies under `Any` alone. Where only the lower end is a container, the upper end's element, key and
//! value are `Any`. The argument's lower end decides the refusals: a list or a string refuses one
//! lying outside `Number | {}`, a record naming a bound outside `Number`, and a record naming
//! neither bound; a dict refuses a key outside `Str | Number | Bool` or outside its key type, a
//! variable's read at its bound's key, which every binding's lies under. Each refusing check is
//! laid as a binding's ([`SiteRule::ReadRefusal`](crate::scope::SiteRule::ReadRefusal)). The
//! read's upper end is `Result {Ok = O, Error = E}`: an index reads the element (`Str` for a
//! string), a slice the container's own type, either `E = Number`; an argument that may be either
//! reads their union. Code reads `Code` either way: a part or a slice is code no load can type, so
//! the run carries it unbuilt. A lookup reads the value type, `E` the key type where the argument's upper
//! end lies under it and the union of both otherwise. Its lower end is `Never`: the run answers
//! either variant.

use crate::elaborate::applied_union;
use crate::elaborate::judge::{self, Marks};
use crate::memory::{BumpVec, collect};
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{Question, Rule, ShapeError, SiteRule, Store};
use crate::symbols::{BinderSymbol, TypeSymbol};
use crate::type_lattice::{DeclaredType, Interval, KType, Parametric, Step, TypeNode};

use super::super::builtins::result_type;
use super::super::lookup::bound;
use super::Bound;

/// The `Result` parameter `name` names, as a construction's argument.
pub(super) fn variant(name: &str) -> BinderSymbol {
    BinderSymbol::Type(TypeSymbol::classify(name).expect("a Type token"))
}

/// What one end of a read's head type is.
#[derive(Clone, Copy)]
enum Container {
    List {
        element: Parametric,
    },
    Dict {
        key: Parametric,
        value: Parametric,
        /// The key of a variable's bound, where the dict is read through one: what its key
        /// refusal reads, since every binding's key lies under it.
        bound_key: Option<KType>,
    },
    Str,
    Code,
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>>
    super::Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// What `end` is as a read's head, read off its node: `None` for anything but a list, a dict,
    /// `Str` or code, or a lexical variable bounded by one.
    fn container(&self, end: Parametric) -> Option<Container> {
        let (types, scratch) = (self.types, self.scratch);
        let piece = |step| judge::projection(types, scratch, end, step);
        if let Some(element) = piece(Step::Element) {
            return Some(Container::List {
                element: element.ty,
            });
        }
        if let (Some(key), Some(value)) = (piece(Step::Key), piece(Step::Value)) {
            return Some(Container::Dict {
                key: key.ty,
                value: value.ty,
                bound_key: Some(key.bound),
            });
        }
        if judge::rigid_within(types, scratch, end, KType::STR) {
            return Some(Container::Str);
        }
        if judge::rigid_within(types, scratch, end, KType::ANY_CODE) {
            return Some(Container::Code);
        }
        if judge::is(self, self.types, self.scratch, end, KType::STR) {
            return Some(Container::Str);
        }
        // A variable is no container but through its bound's pieces, above: any other is read by
        // what a binding gives it ([`Rule::Application`](crate::scope::Rule::Application)).
        let node = judge::node_unless_variable(types, end)?;
        // Code where every binding puts it under `Code`.
        let any_code = KType::ANY_CODE.into();
        let code = judge::under_decided(self, types, scratch, end, any_code) == Some(true);
        if code && !judge::is(self, self.types, self.scratch, end, KType::NEVER) {
            return Some(Container::Code);
        }
        match node {
            TypeNode::List { element } => Some(Container::List { element }),
            TypeNode::Dict { key, value } => Some(Container::Dict {
                key,
                value,
                bound_key: None,
            }),
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
        (_, typed): (&'graph ExpressionPart<'graph>, Bound),
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
                bound_key: None,
            },
            (None, Some(Container::Str)) => Container::Str,
            (None, Some(Container::Code)) => Container::Code,
            (None, None) => return Ok(None),
        };
        let payload = self.part(level, argument)?;
        let (types, scratch) = (self.types, self.scratch);
        let marks: &dyn Marks = &*self;
        // Each check that may refuse is asked, so a binding refuses in the same words.
        let mut asks = BumpVec::with_capacity_in(4, scratch);
        let mut outside = |value: Interval, bound: Parametric| {
            asks.push(Question::LowerEndOutside {
                value,
                bound: bound.into(),
            });
            judge::lower_end_outside(marks, types, scratch, value, bound)
        };
        let number: Parametric = KType::NUMBER.into();
        let union = |members: &[Parametric]| types.union_of(scratch, members);
        let bounds = |end: Parametric| match judge::node_unless_variable(types, end)? {
            TypeNode::Record { fields } => {
                let named = |name| fields.get(bound(name));
                Some([named("from"), named("to")])
            }
            _ => None,
        };
        // A variable's binding decides whether its bounds refuse ([`Rule::SliceBounds`]).
        let variable = judge::node_unless_variable(types, payload.lower).is_none();
        let operands = [DeclaredType::Type(payload.lower)];
        let unbounded = variable
            && judge::defined(marks, types, scratch, Rule::SliceBounds, &operands).0 == Some(false);
        let refused = match shape {
            Container::Dict { key, bound_key, .. } => {
                let scalar = [KType::STR, KType::NUMBER, KType::BOOL].map(Parametric::from);
                outside(payload, union(&scalar))
                    || outside(payload, bound_key.map_or(key, Parametric::from))
            }
            _ => {
                outside(payload, union(&[number, KType::EMPTY_RECORD.into()]))
                    || unbounded
                    || bounds(payload.lower).is_some_and(|named| {
                        named.iter().all(Option::is_none)
                            || (named.iter().flatten())
                                .any(|bound| outside(Interval::point(*bound), number))
                    })
            }
        };
        let refusal = ShapeError::ReadNeverSatisfied {
            container: typed.upper,
            argument: payload.upper,
            member: None,
            at: node.source,
        };
        if refused {
            return Err(refusal);
        }
        if variable {
            asks.push(Question::Defined {
                rule: Rule::SliceBounds,
                operands: collect(scratch, operands.into_iter()),
            });
        }
        let mut refusals = BumpVec::with_capacity_in(asks.len(), scratch);
        refusals.resize(asks.len(), refusal);
        let site = self.node_site(level, node);
        self.lay_refusals(level, site, SiteRule::ReadRefusal, &asks, &refusals);
        // Each end a binding decides is the decided name on its door's question, so the read's
        // type is the same at load and under every binding.
        let (ok, error) = match shape {
            Container::Dict {
                key,
                value,
                bound_key,
            } => {
                // A variable's key is no closed type, so whether the argument lies under it is
                // *fits*, by the rigid rule; a written key's is the order, decided once closed.
                let (a, b) = (payload.upper, key);
                let under = match bound_key {
                    Some(_) => (
                        Question::Fits {
                            a: a.into(),
                            b: b.into(),
                        },
                        judge::fit_decided(self, types, scratch, a, b),
                    ),
                    None => (
                        Question::Under { a, b },
                        judge::under_decided(self, types, scratch, a, b),
                    ),
                };
                let wide = union(&[key, payload.upper]);
                (
                    value,
                    judge::on_answer(self, types, scratch, under, (wide, key)),
                )
            }
            _ => {
                let element = match shape {
                    Container::List { element } => element,
                    Container::Code => KType::ANY_CODE.into(),
                    _ => KType::STR.into(),
                };
                let head_upper = match (shape, self.container(typed.upper)) {
                    (Container::Code, _) => KType::ANY_CODE.into(),
                    (_, Some(_)) => typed.upper,
                    (_, None) => KType::ANY.into(),
                };
                let indexed = |constant: KType| {
                    let question = Question::Same {
                        a: payload.upper.into(),
                        b: Parametric::from(constant).into(),
                    };
                    (
                        question,
                        judge::is_decided(self, types, scratch, payload.upper, constant),
                    )
                };
                let by_number = indexed(KType::NUMBER);
                let ok = match by_number.1 {
                    Some(true) => element,
                    _ => {
                        // A record the bounds admit is a slice; whether a variable's binding is
                        // one is the order's to decide, so a binding slices as a fresh load does.
                        let record: Parametric = KType::EMPTY_RECORD.into();
                        let by_record = (
                            Question::Under {
                                a: payload.upper,
                                b: record,
                            },
                            judge::under_decided(self, types, scratch, payload.upper, record),
                        );
                        let wide = union(&[element, head_upper]);
                        let sliced =
                            judge::on_answer(self, types, scratch, by_record, (wide, head_upper));
                        let by_never = indexed(KType::NEVER);
                        let past =
                            judge::on_answer(self, types, scratch, by_never, (sliced, element));
                        judge::on_answer(self, types, scratch, by_number, (past, element))
                    }
                };
                (ok, number)
            }
        };
        let mut arguments = BumpVec::with_capacity_in(2, scratch);
        arguments.extend([(variant("Ok"), ok), (variant("Error"), error)]);
        arguments.sort_unstable_by_key(|(name, _)| *name);
        let upper = applied_union(types, scratch, result_type(self.builtins), &arguments);
        Ok(Some(Interval {
            lower: KType::NEVER.into(),
            upper,
        }))
    }
}
