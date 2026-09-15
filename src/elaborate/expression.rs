//! One type expression, part by part: a name read through the activation, a composite built from
//! the handles its parts elaborate to.

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::forms::{FormId, KEYWORDS};
use crate::parse::{ExpressionPart, KExpression, KeywordSymbol, StaticName, TypeSymbol};
use crate::scope::{Activation, Binding, Site, pair_name};
use crate::type_lattice::{DispatchTokenElement, KType, TypeRegistry};
use crate::values::{Callable, Value};

use super::Elaboration;

/// The connector keywords of the formless composites.
struct Connectors {
    list: StaticName<KeywordSymbol>,
    of: StaticName<KeywordSymbol>,
    map: StaticName<KeywordSymbol>,
    union: StaticName<KeywordSymbol>,
}

static CONNECTORS: Connectors = Connectors {
    list: crate::static_name!(KeywordSymbol, "LIST"),
    of: crate::static_name!(KeywordSymbol, "OF"),
    map: crate::static_name!(KeywordSymbol, "MAP"),
    union: crate::static_name!(KeywordSymbol, "|"),
};

/// `part` as a type, its names read through `reader`: a name in `quantifiers` is that group's
/// quantifier at its position, and every other name is the mention `reader`'s shape recorded at its
/// site, which must read as a type.
pub fn type_expression<'graph, X: Callable>(
    part: &ExpressionPart<'graph>,
    reader: &Activation<'graph, '_, X>,
    quantifiers: &[TypeSymbol],
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<KType, Elaboration> {
    let groups = Groups {
        names: quantifiers,
        outer: None,
    };
    Elaborator {
        reader,
        types,
        scratch,
    }
    .part(part, &groups)
}

/// The `FOR ALL` groups enclosing a part, innermost first.
#[derive(Clone, Copy)]
pub(super) struct Groups<'q> {
    pub(super) names: &'q [TypeSymbol],
    pub(super) outer: Option<&'q Groups<'q>>,
}

/// Where a name sits among the enclosing groups.
enum Quantifier {
    /// The innermost group's quantifier at this position.
    Innermost(usize),
    /// An enclosing group's, read under a nested shape whose own group shadows it.
    Shadowed,
    /// No group's: a mention.
    Free,
}

impl Groups<'_> {
    fn find(&self, name: TypeSymbol) -> Quantifier {
        if let Some(index) = self.names.iter().position(|declared| *declared == name) {
            return Quantifier::Innermost(index);
        }
        let mut outer = self.outer;
        while let Some(group) = outer {
            if group.names.contains(&name) {
                return Quantifier::Shadowed;
            }
            outer = group.outer;
        }
        Quantifier::Free
    }
}

/// What elaborating one expression reads through.
pub(super) struct Elaborator<'e, 'run, 'graph, 'cell, 'x, X> {
    pub(super) reader: &'e Activation<'graph, 'cell, X>,
    pub(super) types: &'e TypeRegistry<'run>,
    pub(super) scratch: BumpAllocator<'x>,
}

impl<'graph, X: Callable> Elaborator<'_, '_, 'graph, '_, '_, X> {
    pub(super) fn part(
        &self,
        part: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<KType, Elaboration> {
        let site = Site::of(part);
        match part {
            ExpressionPart::Type(name) => self.name(site, *name, groups),
            ExpressionPart::Expression(node) | ExpressionPart::SigiledTypeExpr(node) => {
                self.node(site, node.reference(), groups)
            }
            ExpressionPart::RecordType(node) => {
                let mut fields = BumpVec::new_in(self.scratch);
                self.pairs(site, node.reference(), groups, |name, ktype| {
                    fields.push((name, ktype));
                    Ok(())
                })?;
                Ok(self.types.record(self.scratch, &fields))
            }
            _ => Err(Elaboration::Unsupported { site }),
        }
    }

    /// A type name: a quantifier of the innermost group, or the type its mention reads.
    fn name(
        &self,
        site: Site,
        name: TypeSymbol,
        groups: &Groups<'_>,
    ) -> Result<KType, Elaboration> {
        match groups.find(name) {
            Quantifier::Innermost(index) => return Ok(self.types.quantified(index, KType::ANY)),
            Quantifier::Shadowed => return Err(Elaboration::Unsupported { site }),
            Quantifier::Free => {}
        }
        let mention = self
            .reader
            .shape()
            .mention(site)
            .expect("the shape builder records every type name a type expression reads");
        match self.reader.read(mention.coordinate) {
            Binding::Bound(Value::Type(value)) => Ok(value.handle()),
            Binding::Bound(_) => Err(Elaboration::NotAType { name, site }),
            Binding::Pending(binder) => Err(Elaboration::Pending { name, binder }),
        }
    }

    /// A parenthesized or sigiled type expression.
    fn node(
        &self,
        site: Site,
        node: &KExpression<'graph>,
        groups: &Groups<'_>,
    ) -> Result<KType, Elaboration> {
        let unsupported = Elaboration::Unsupported { site };
        let parts = node.parts;
        if let [only] = parts {
            return self.part(&only.value, groups);
        }
        if let Some(form) = node.cache().form() {
            let part = |index: usize| &parts[index].value;
            return match form.id {
                FormId::LambdaType => self.function(part(1), part(3), groups),
                FormId::ExpressionHead => self.shape(&[], part(1), part(3), groups),
                FormId::QuantifiedExpressionHead => {
                    let names = quantifiers(part(3), self.scratch);
                    self.shape(&names, part(4), part(6), groups)
                }
                FormId::Attribute => {
                    let union = self.part(part(1), groups)?;
                    let tag = match part(2) {
                        ExpressionPart::Type(name) => name.symbol(),
                        ExpressionPart::Identifier(name) => name.symbol(),
                        _ => return Err(unsupported),
                    };
                    self.types
                        .union_member_named(union, tag)
                        .ok_or(Elaboration::NoSuchMember { union, tag })
                }
                _ => Err(unsupported),
            };
        }
        let keyword = |index: usize, expected: &StaticName<KeywordSymbol>| matches!(parts[index].value, ExpressionPart::Keyword(symbol) if symbol == expected.symbol());
        match parts.len() {
            3 if keyword(0, &CONNECTORS.list) && keyword(1, &CONNECTORS.of) => {
                Ok(self.types.list(self.part(&parts[2].value, groups)?))
            }
            4 if keyword(0, &CONNECTORS.map) && keyword(2, &KEYWORDS.arrow) => {
                let key = self.part(&parts[1].value, groups)?;
                let value = self.part(&parts[3].value, groups)?;
                Ok(self.types.dict(key, value))
            }
            len if len % 2 == 1
                && (1..len)
                    .step_by(2)
                    .all(|index| keyword(index, &CONNECTORS.union)) =>
            {
                let mut members = BumpVec::with_capacity_in(len / 2 + 1, self.scratch);
                for member in parts.iter().step_by(2) {
                    members.push(self.part(&member.value, groups)?);
                }
                Ok(self.types.union_of(self.scratch, &members))
            }
            _ => Err(unsupported),
        }
    }

    /// `FN <schema> -> <return>`.
    pub(super) fn function(
        &self,
        schema: &ExpressionPart<'graph>,
        ret: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<KType, Elaboration> {
        let ExpressionPart::RecordType(fields) = schema else {
            return Err(Elaboration::Unsupported {
                site: Site::of(schema),
            });
        };
        let mut params = BumpVec::new_in(self.scratch);
        self.pairs(
            Site::of(schema),
            fields.reference(),
            groups,
            |name, ktype| {
                params.push((name, ktype));
                Ok(())
            },
        )?;
        let ret = self.part(ret, groups)?;
        Ok(self.types.function_type(self.scratch, &params, ret))
    }

    /// `EXPR [FOR ALL <names>] <head> -> <return>`: the head's keywords and typed slots, under a
    /// group of its own.
    pub(super) fn shape(
        &self,
        names: &[TypeSymbol],
        head: &ExpressionPart<'graph>,
        ret: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<KType, Elaboration> {
        let own = Groups {
            names,
            outer: Some(groups),
        };
        let unsupported = Elaboration::Unsupported {
            site: Site::of(head),
        };
        let ExpressionPart::Expression(run) = head else {
            return Err(unsupported);
        };
        let run = run.reference();
        let mut elements = BumpVec::with_capacity_in(run.parts.len(), self.scratch);
        let mut index = 0;
        while index < run.parts.len() {
            match (run.parts[index].value, pair_name(run, index)) {
                (_, Some(_)) => {
                    let slot = self.part(&run.parts[index + 1].value, &own)?;
                    elements.push(DispatchTokenElement::Slot(slot));
                    index += 2;
                }
                (ExpressionPart::Keyword(symbol), None) => {
                    elements.push(DispatchTokenElement::Keyword(symbol));
                    index += 1;
                }
                _ => return Err(unsupported),
            }
        }
        let ret = self.part(ret, &own)?;
        Ok(self
            .types
            .shape_type(self.scratch, names, &elements, ret)
            .handle)
    }

    /// Each `<name> :<Type>` pair of a field list, its type elaborated; a `_` pair or anything
    /// that is not a pair is unsupported.
    fn pairs(
        &self,
        site: Site,
        run: &KExpression<'graph>,
        groups: &Groups<'_>,
        mut field: impl FnMut(crate::parse::BinderSymbol, KType) -> Result<(), Elaboration>,
    ) -> Result<(), Elaboration> {
        let mut index = 0;
        while index < run.parts.len() {
            let Some(Some(name)) = pair_name(run, index) else {
                return Err(Elaboration::Unsupported { site });
            };
            field(name, self.part(&run.parts[index + 1].value, groups)?)?;
            index += 2;
        }
        Ok(())
    }
}

/// The names a `FOR ALL` group declares, in written order.
pub(super) fn quantifiers<'x>(
    group: &ExpressionPart<'_>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, TypeSymbol> {
    let mut names = BumpVec::new_in(scratch);
    if let ExpressionPart::Expression(group) = group {
        names.extend(group.parts.iter().filter_map(|part| match part.value {
            ExpressionPart::Type(name) => Some(name),
            _ => None,
        }));
    }
    names
}
