//! A body's **code digest**: its code as resolved, not as written, computed where the draft is
//! built — the chain of drafts enclosing it is still at hand, so a read is followed to the binding
//! it lands at.
//!
//! The digest is the shape's kind, its parameters, then each statement part by part, every nested
//! body hashed in place by its own code digest, so a body's digest never depends on where it sits
//! in the program:
//!
//! - a keyword hashes its symbol;
//! - a literal hashes as syntax, which is its value: a scalar its payload under its kind's tag — a
//!   number's bits, a bool, `null`, a string length-prefixed — and a container literal its parts
//!   under a group tag with their count, a record literal's fields in symbol order, so field order
//!   is blind, and a dict literal's pairs as written;
//! - a name hashes its resolution: a builtin by its index, a read landing at the program's top
//!   level by that binding's slot, a local by its hops and slot, and any other capture by its hops
//!   and capture slot; a name written where it is declared hashes its symbol;
//! - a keyworded use hashes its key and each candidate by the same rule, a spread tagged;
//! - a type expression hashes its syntax, its names resolved the same way;
//! - spans are left out, and marks are kept.
//!
//! A top-level read names the binding it reads, which is bound once per program, so two bodies of
//! one text whose keyworded uses resolve to different candidates digest apart. A `MODULE` or
//! `GROUP` body's digest then hashes each top-level binding its `OVER` list names that the code —
//! nested bodies included — did not already name, so listing a name the body reads changes
//! nothing, and listing one it does not read brands it.
//!
//! The same pass records which captures read the program's top level: the run reads those where
//! they live, so a closure's or a module's digest composes only the others
//! ([`BodyShape::composed_captures`](super::super::BodyShape::composed_captures)).

use crate::memory::BumpVec;
use crate::parse::{ExpressionPart, KExpression, KLiteral, Mark};
use crate::values::digest::{DigestHasher, Tag};

use super::super::{
    Candidate, CaptureSource, Coordinate, Position, ShapeKind, Site, Slot, Target, TopLevel,
};
use super::Builder;

impl<'graph> Builder<'graph, '_, '_> {
    /// Digest the draft at `level` over `statements` — its own, or a refused quote's code as
    /// written — once every statement is walked and every child built; record its code digest, the
    /// top-level bindings it names, nested bodies' included, and which of its captures read the
    /// top level.
    pub(super) fn digest_draft(&mut self, level: usize, statements: &[KExpression<'graph>]) {
        let draft = &mut self.chain[level];
        draft.mentions.sort_unstable_by_key(|mention| mention.site);
        draft.candidates.sort_unstable_by_key(|(site, _)| *site);
        draft.children.sort_unstable_by_key(|(site, _)| *site);
        let mut top = BumpVec::with_capacity_in(draft.captures.len(), self.scratch);
        for capture in self.chain[level].captures.iter() {
            let landed = match capture.source {
                CaptureSource::Read(read) => level
                    .checked_sub(1)
                    .and_then(|outer| self.landing(outer, read)),
                _ => None,
            };
            top.push(self.top_slot(landed));
        }
        let draft = &self.chain[level];
        let mut walk = Walk {
            builder: self,
            level,
            named: BumpVec::new_in(self.scratch),
        };
        let mut hasher = DigestHasher::new(Tag::Shape);
        hasher.feed(kind_tag(draft.kind));
        let names = draft.channels();
        let parameters = (0..names.len())
            .filter(|index| names.get(*index) == Position::PARAMETER)
            .count();
        hasher.count(parameters);
        for index in (0..names.len()).filter(|index| names.get(*index) == Position::PARAMETER) {
            hasher.feed(names.name(index));
        }
        hasher.count(statements.len());
        for statement in statements {
            walk.node(&mut hasher, statement);
        }
        let mut named = walk.named;
        for (_, child) in draft.children.iter() {
            named.extend(child.named_top.iter().copied());
        }
        named.sort_unstable();
        named.dedup();
        for listed in draft.listed_top.iter() {
            if named.binary_search(listed).is_err() {
                hasher.tag(Tag::Top).feed(listed);
            }
        }
        let code = hasher.finished();
        let draft = &mut self.chain[level];
        draft.code = code;
        draft.named_top = named;
        draft.top_captures = top;
    }

    /// The top-level slot a landing is, where it lands at the program's own draft.
    fn top_slot(&self, landed: Option<(usize, Target)>) -> Option<Slot> {
        match landed {
            Some((0, Target::Local(slot))) if self.chain[0].kind == ShapeKind::Program => {
                Some(slot)
            }
            _ => None,
        }
    }
}

/// One draft's digest walk: the builder whose chain it reads, the draft's level, and each top-level
/// binding its code names.
struct Walk<'a, 'graph, 'x, 'e> {
    builder: &'a Builder<'graph, 'x, 'e>,
    level: usize,
    named: BumpVec<'x, TopLevel>,
}

impl<'graph> Walk<'_, 'graph, '_, '_> {
    /// A node: a keyworded use's key and candidates where it is one, then its parts in order.
    fn node(&mut self, hasher: &mut DigestHasher, node: &KExpression<'graph>) {
        let draft = &self.builder.chain[self.level];
        let site = Site::of_node(node);
        if let Ok(index) = draft
            .candidates
            .binary_search_by_key(&site, |(site, _)| *site)
        {
            let list = draft.candidates[index].1;
            hasher
                .tag(Tag::Use)
                .feed(list.key)
                .count(list.candidates.len());
            for candidate in list.candidates {
                match candidate {
                    Candidate::One(coordinate) => self.name(hasher, *coordinate),
                    Candidate::Spread(coordinate) => {
                        hasher.tag(Tag::Spread);
                        self.name(hasher, *coordinate);
                    }
                }
            }
        }
        hasher.tag(Tag::Group).count(node.parts.len());
        for part in node.parts {
            self.part(hasher, &part.value);
        }
    }

    /// One part, by its kind.
    fn part(&mut self, hasher: &mut DigestHasher, part: &ExpressionPart<'graph>) {
        let draft = &self.builder.chain[self.level];
        let site = Site::of(part);
        if let Ok(index) = draft
            .children
            .binary_search_by_key(&site, |(site, _)| *site)
        {
            hasher.tag(Tag::Nested).digest(draft.children[index].1.code);
            return;
        }
        match part {
            ExpressionPart::Keyword(symbol) => {
                hasher.tag(Tag::Keyword).feed(symbol);
            }
            ExpressionPart::Identifier(_) | ExpressionPart::Type(_) => self.leaf(hasher, part),
            ExpressionPart::MarkedName(mark, _) => {
                hasher.tag(Tag::Mark).feed(mark_tag(*mark));
                self.leaf(hasher, part);
            }
            ExpressionPart::Expression(node)
            | ExpressionPart::SigiledTypeExpr(node)
            | ExpressionPart::RecordType(node)
            | ExpressionPart::QuotedExpression(node) => {
                hasher.tag(Tag::Group).feed(part_tag(part));
                self.node(hasher, node.reference());
            }
            ExpressionPart::MarkedUse(mark, node) => {
                hasher.tag(Tag::Mark).feed(mark_tag(*mark));
                self.node(hasher, node.reference());
            }
            ExpressionPart::Literal(literal) => scalar(hasher, literal),
            ExpressionPart::ListLiteral(items) => {
                hasher
                    .tag(Tag::Group)
                    .feed(part_tag(part))
                    .count(items.len());
                for item in items.iter() {
                    self.part(hasher, item);
                }
            }
            ExpressionPart::DictLiteral(pairs) => {
                hasher
                    .tag(Tag::Group)
                    .feed(part_tag(part))
                    .count(pairs.len());
                for (key, value) in pairs.iter() {
                    self.part(hasher, key);
                    self.part(hasher, value);
                }
            }
            ExpressionPart::RecordLiteral(fields) => {
                hasher
                    .tag(Tag::Group)
                    .feed(part_tag(part))
                    .count(fields.len());
                let mut sorted = BumpVec::with_capacity_in(fields.len(), self.builder.scratch);
                sorted.extend_from_slice(fields);
                sorted.sort_unstable_by_key(|(name, _)| name.symbol());
                for (name, value) in sorted.iter() {
                    hasher.feed(name);
                    self.part(hasher, value);
                }
            }
        }
    }

    /// A name: its resolution where the shape records a mention of it, and its symbol where it is
    /// written as it is declared.
    fn leaf(&mut self, hasher: &mut DigestHasher, part: &ExpressionPart<'graph>) {
        let draft = &self.builder.chain[self.level];
        let site = Site::of(part);
        match draft
            .mentions
            .binary_search_by_key(&site, |mention| mention.site)
        {
            Ok(index) => self.name(hasher, draft.mentions[index].coordinate),
            Err(_) => {
                let symbol = match part {
                    ExpressionPart::Identifier(name) => name.symbol(),
                    ExpressionPart::Type(name) => name.symbol(),
                    ExpressionPart::MarkedName(_, name) => name.symbol(),
                    _ => unreachable!("a leaf is a name"),
                };
                hasher.tag(Tag::Keyword).feed(symbol);
            }
        }
    }

    /// A read's resolution: a builtin by its index, a read landing at the program's top level by
    /// the binding's slot, and any other read by its coordinate.
    fn name(&mut self, hasher: &mut DigestHasher, coordinate: Coordinate) {
        let Coordinate::Activation { hops, target } = coordinate else {
            let Coordinate::Builtin(index) = coordinate else {
                unreachable!("a coordinate is a builtin or an activation's")
            };
            hasher.tag(Tag::Builtin).feed(index.0);
            self.named.push(TopLevel::Builtin(index));
            return;
        };
        let landed = self.builder.landing(self.level, coordinate);
        if let Some(slot) = self.builder.top_slot(landed) {
            hasher.tag(Tag::Top).feed(slot.0);
            self.named.push(TopLevel::Root(slot));
            return;
        }
        match target {
            Target::Local(slot) => hasher.tag(Tag::Local).feed(hops).feed(slot.0),
            Target::Capture(capture) => hasher.tag(Tag::Capture).feed(hops).feed(capture.0),
        };
    }
}

/// A scalar literal as syntax: its kind's tag, then its payload.
fn scalar(hasher: &mut DigestHasher, literal: &KLiteral<'_>) {
    match literal {
        KLiteral::Number(number) => hasher.tag(Tag::Number).feed(number.to_bits()),
        KLiteral::String(text) => hasher.tag(Tag::Str).text(text.as_bytes()),
        KLiteral::Boolean(flag) => hasher.tag(Tag::Bool).feed(flag),
        KLiteral::Null => hasher.tag(Tag::Null),
    };
}

/// A shape kind's place in the digest.
fn kind_tag(kind: ShapeKind) -> u8 {
    match kind {
        ShapeKind::Program => 0,
        ShapeKind::Callable => 1,
        ShapeKind::Module => 2,
        ShapeKind::Block => 3,
        ShapeKind::Code => 4,
    }
}

/// A mark's place in the digest.
fn mark_tag(mark: Mark) -> u8 {
    match mark {
        Mark::Written => 0,
        Mark::Built => 1,
    }
}

/// A grouping part's place in the digest.
fn part_tag(part: &ExpressionPart<'_>) -> u8 {
    match part {
        ExpressionPart::Expression(_) => 0,
        ExpressionPart::SigiledTypeExpr(_) => 1,
        ExpressionPart::RecordType(_) => 2,
        ExpressionPart::QuotedExpression(_) => 3,
        ExpressionPart::ListLiteral(_) => 4,
        ExpressionPart::DictLiteral(_) => 5,
        ExpressionPart::RecordLiteral(_) => 6,
        _ => 7,
    }
}
