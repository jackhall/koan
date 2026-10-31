//! The syntax copier: a whole expression laid again through a writer, every part and every nested
//! quote with it, its strings too. Code composed at run time owns its syntax this way — its
//! composition copies the code value's syntax into its own storage and builds its shapes over the
//! copy, and a code value's composed syntax is laid in a region the same way — so nothing a copy
//! holds borrows the storage it came from.
//!
//! A copy may also rewrite the scope each `$` or filled mark carries, through a [`Remark`] told
//! where the mark stands: whether a quote value lies between the copy's root and it, in the syntax
//! it came from and in the copy. The remark may also fill: mark a name filled, or hold a node under
//! a filled use, as a `USING` does ([knot/code.rs](../knot/code.rs)). A code view's materializer lays its stretches this way
//! ([knot/view.rs](../knot/view.rs)), which is also why a part copied on its own is told whether
//! its slot is read as written before and now.
//! Modeled on the shape builder's rewrite walk (`scope/shape/build/rewrite.rs`), which copies only
//! the path to a change and never enters a quote; this copies everything.

use crate::memory::{Bump, BumpVec, Writer, collect, resident};
use crate::source::Spanned;

use crate::symbols::{BinderSymbol, KeySymbol};

use super::{ExpressionPart, KExpression, KLiteral, KTemplate, ListItem, Mark, Scope};

/// Where a copied part stands: whether a quote value lies between the copy's root and it in the
/// syntax it came from, and in the copy.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Quoted {
    pub before: bool,
    pub now: bool,
}

/// What a copy does to the scope each `$` or filled mark carries, and where it fills.
pub trait Remark {
    fn scope(&mut self, scope: Scope, at: Quoted) -> Scope;

    /// The filled mark `part`, a name standing at `at`, becomes, where the copy fills it.
    fn filled(&mut self, _part: &ExpressionPart<'_>, _at: Quoted) -> Option<Mark> {
        None
    }

    /// The scope of the filled use `node`, standing at `at`, is held under where the copy fills a
    /// key of it, each key it fills handed to `keys`.
    fn filled_use(
        &mut self,
        _node: &KExpression<'_>,
        _at: Quoted,
        _keys: &mut dyn FnMut(KeySymbol),
    ) -> Option<Scope> {
        None
    }
}

/// A plain copy's remark: every scope as it was.
struct Unchanged;

impl Remark for Unchanged {
    fn scope(&mut self, scope: Scope, _: Quoted) -> Scope {
        scope
    }
}

/// `syntax` laid whole through `writer`: equal to it as code, borrowing nothing of its storage.
pub fn copied<'a>(writer: Writer<'a, '_>, syntax: &KExpression<'_>) -> &'a KExpression<'a> {
    remarked_whole(writer, syntax, &mut Unchanged)
}

/// `syntax` laid whole through `writer`, each mark's scope rewritten and each name and use filled
/// as `remark` says.
pub fn remarked_whole<'a>(
    writer: Writer<'a, '_>,
    syntax: &KExpression<'_>,
    remark: &mut dyn Remark,
) -> &'a KExpression<'a> {
    let scratch = Bump::new();
    Copier {
        writer,
        scratch: &scratch,
        remark,
    }
    .node(syntax, Quoted::default())
}

/// `part`, standing at `at`, laid through `writer` with each mark's scope rewritten by `remark`:
/// a part of a node being laid anew, `written` saying whether its slot is read as written in the
/// node it came from and in the node it lands in.
pub fn remarked<'a>(
    writer: Writer<'a, '_>,
    part: &ExpressionPart<'_>,
    written: (bool, bool),
    at: Quoted,
    remark: &mut dyn Remark,
) -> ExpressionPart<'a> {
    let scratch = Bump::new();
    Copier {
        writer,
        scratch: &scratch,
        remark,
    }
    .part(part, written, at)
}

/// One copy's destination writer, the scratch its runs gather in, and its remark.
struct Copier<'a, 'w, 'x, 'r> {
    writer: Writer<'a, 'w>,
    scratch: &'x Bump,
    remark: &'r mut dyn Remark,
}

impl<'a> Copier<'a, '_, '_, '_> {
    fn node(&mut self, node: &KExpression<'_>, at: Quoted) -> &'a KExpression<'a> {
        let mut keys = BumpVec::new_in(self.scratch);
        let filled = self.remark.filled_use(node, at, &mut |key| keys.push(key));
        let slots = node.written_slots();
        let mut run = BumpVec::with_capacity_in(node.parts.len(), self.scratch);
        for (index, part) in node.parts.iter().enumerate() {
            let written = slots.at(index);
            run.push(Spanned {
                value: self.part(&part.value, (written, written), at),
                span: part.span,
            });
        }
        let copy = KExpression::nested(self.writer, &run, node.source);
        // A filled use stands where the node stood, held by a node of its own, so the part that
        // held the node still holds a node: a statement stays a group.
        let Some(scope) = filled else {
            return copy;
        };
        let keys = collect(self.writer, keys.iter().copied());
        let holder = [Spanned {
            value: ExpressionPart::FilledUse(scope, keys, copy),
            span: Some(node.source.span),
        }];
        KExpression::nested(self.writer, &holder, node.source)
    }

    /// `mark` with its scope remarked, standing at `at`.
    fn mark(&mut self, mark: Mark, at: Quoted) -> Mark {
        match mark {
            Mark::Written(scope) => Mark::Written(self.remark.scope(scope, at)),
            Mark::Filled(scope) => Mark::Filled(self.remark.scope(scope, at)),
            Mark::Built => Mark::Built,
        }
    }

    fn part(
        &mut self,
        part: &ExpressionPart<'_>,
        written: (bool, bool),
        at: Quoted,
    ) -> ExpressionPart<'a> {
        let writer = self.writer;
        if let Some(mark) = self.remark.filled(part, at) {
            let name = match part {
                ExpressionPart::Identifier(name) => BinderSymbol::Value(*name),
                ExpressionPart::Type(name) => BinderSymbol::Type(*name),
                ExpressionPart::MarkedName(_, name) => *name,
                _ => unreachable!("a copy fills a name alone"),
            };
            return ExpressionPart::MarkedName(mark, name);
        }
        match part {
            ExpressionPart::Keyword(keyword) => ExpressionPart::Keyword(*keyword),
            ExpressionPart::Identifier(name) => ExpressionPart::Identifier(*name),
            ExpressionPart::Type(name) => ExpressionPart::Type(*name),
            ExpressionPart::Expression(node) => ExpressionPart::Expression(self.node(node, at)),
            ExpressionPart::SigiledTypeExpr(node) => {
                ExpressionPart::SigiledTypeExpr(self.node(node, at))
            }
            ExpressionPart::RecordType(node) => ExpressionPart::RecordType(self.node(node, at)),
            // A quote no builtin reads as written is a quote value.
            ExpressionPart::QuotedExpression(node) => {
                let at = Quoted {
                    before: at.before || !written.0,
                    now: at.now || !written.1,
                };
                ExpressionPart::QuotedExpression(self.node(node, at))
            }
            ExpressionPart::MarkedName(mark, name) => {
                ExpressionPart::MarkedName(self.mark(*mark, at), *name)
            }
            ExpressionPart::MarkedUse(mark, node) => {
                let mark = self.mark(*mark, at);
                ExpressionPart::MarkedUse(mark, self.node(node, at))
            }
            ExpressionPart::FilledUse(scope, keys, node) => ExpressionPart::FilledUse(
                self.remark.scope(*scope, at),
                collect(writer, keys.iter().copied()),
                self.node(node, at),
            ),
            ExpressionPart::Splice(name) => ExpressionPart::Splice(*name),
            // A literal's items stand in its slot, read as it is.
            ExpressionPart::ListLiteral(items) => {
                let mut run = BumpVec::with_capacity_in(items.len(), self.scratch);
                for item in items.iter() {
                    run.push(match item {
                        ListItem::Part(part) => ListItem::Part(self.part(part, written, at)),
                        ListItem::Spread(part) => ListItem::Spread(self.part(part, written, at)),
                    });
                }
                ExpressionPart::ListLiteral(collect(writer, run.iter().copied()))
            }
            ExpressionPart::Template(template) => {
                let mut slots = BumpVec::with_capacity_in(template.slots.len(), self.scratch);
                for slot in template.slots.iter() {
                    slots.push(slot.with(self.part(slot.part(), written, at)));
                }
                let mut pieces = BumpVec::with_capacity_in(template.pieces.len(), self.scratch);
                pieces.extend(template.pieces.iter().map(|piece| writer.text(piece)));
                ExpressionPart::Template(resident(
                    writer,
                    KTemplate {
                        pieces: collect(writer, pieces.iter().copied()),
                        slots: collect(writer, slots.iter().copied()),
                    },
                ))
            }
            ExpressionPart::DictLiteral(pairs) => {
                let mut run = BumpVec::with_capacity_in(pairs.len(), self.scratch);
                for (key, value) in pairs.iter() {
                    let key = self.part(key, written, at);
                    run.push((key, self.part(value, written, at)));
                }
                ExpressionPart::DictLiteral(collect(writer, run.iter().copied()))
            }
            ExpressionPart::RecordLiteral(pairs) => {
                let mut run = BumpVec::with_capacity_in(pairs.len(), self.scratch);
                for (name, value) in pairs.iter() {
                    run.push((*name, self.part(value, written, at)));
                }
                ExpressionPart::RecordLiteral(collect(writer, run.iter().copied()))
            }
            ExpressionPart::Literal(literal) => ExpressionPart::Literal(match literal {
                KLiteral::Number(number) => KLiteral::Number(*number),
                KLiteral::String(text) => KLiteral::String(writer.text(text)),
                KLiteral::Boolean(boolean) => KLiteral::Boolean(*boolean),
                KLiteral::Null => KLiteral::Null,
            }),
        }
    }
}
