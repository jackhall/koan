//! Lowering a region-pure AST part straight to a value, with no dispatch and no scope.

use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::parse::{ExpressionPart, KLiteral};
use crate::type_lattice::{KType, TypeRegistry};

use super::digest::{ContentDigest, DigestHasher, Tag, composite, contents};
use super::{
    Dict, Key, Knotted, List, Nothing, Record, Value, dict_type, kept_entries, list_type,
    record_type, text,
};

impl<'cell, X: Knotted> Value<'cell, X> {
    /// The value a region-pure part denotes: a scalar or string literal, or a container literal
    /// whose every element lowers and whose every dict key is a scalar literal. `None` for a part
    /// that needs dispatch or a scope — a name, a parenthesized expression, a sigiled type body, a
    /// quote, whose code binds its `$` names where it is written — anywhere inside it; the part is
    /// checked whole before anything is written, so a refusal leaves the region untouched.
    pub fn lower_part(
        writer: Writer<'cell>,
        part: &ExpressionPart<'_>,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> Option<Value<'cell, X>> {
        lowers(part).then(|| lower(writer, part, types, scratch))
    }
}

/// The content digest of the value a region-pure part denotes — what [`Value::lower_part`] would lay
/// down — with nothing laid down: `None` where `lower_part` refuses the part.
pub fn literal_digest(
    part: &ExpressionPart<'_>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Option<ContentDigest> {
    lowers(part).then(|| digested(part, types, scratch).0)
}

/// The digest and the type of the value a part [`lowers`] took, each as its door computes them.
fn digested(
    part: &ExpressionPart<'_>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> (ContentDigest, KType) {
    let scalar = |value: Value<'_>| (value.digest(), value.concrete_ktype());
    match part {
        ExpressionPart::Literal(KLiteral::Number(number)) => scalar(Value::Number(*number)),
        ExpressionPart::Literal(KLiteral::String(literal)) => scalar(Value::Str(literal)),
        ExpressionPart::Literal(KLiteral::Boolean(flag)) => scalar(Value::Bool(*flag)),
        ExpressionPart::Literal(KLiteral::Null) => scalar(Value::Null),
        ExpressionPart::ListLiteral(items) => {
            let mut cells = BumpVec::with_capacity_in(items.len(), scratch);
            cells.extend(items.iter().map(|item| digested(item, types, scratch)));
            let ktype = list_type(types, scratch, cells.iter().map(|(_, ktype)| *ktype));
            let contents = contents(cells.iter().map(|(digest, _)| *digest));
            (composite(Tag::List, ktype, contents), ktype)
        }
        ExpressionPart::DictLiteral(pairs) => {
            let mut entries = BumpVec::with_capacity_in(pairs.len(), scratch);
            entries.extend(pairs.iter().map(|(key, value)| {
                let key = match key {
                    ExpressionPart::Literal(KLiteral::String(literal)) => Key::str(literal),
                    ExpressionPart::Literal(KLiteral::Number(number)) => {
                        Key::number(*number).expect("a lowerable key is not NaN")
                    }
                    ExpressionPart::Literal(KLiteral::Boolean(flag)) => Key::bool(*flag),
                    _ => unreachable!("a lowerable dict key is a scalar literal"),
                };
                (key, digested(value, types, scratch))
            }));
            let kept = kept_entries(&entries, scratch);
            let ktype = dict_type(
                types,
                scratch,
                kept.iter()
                    .map(|at| (entries[*at].0.ktype(), entries[*at].1.1)),
            );
            let mut hasher = DigestHasher::new(Tag::Contents);
            hasher.count(kept.len());
            for at in kept.iter() {
                let (key, (cell, _)) = entries[*at];
                hasher.digest(key.value::<Nothing>().digest()).digest(cell);
            }
            (composite(Tag::Dict, ktype, hasher.finished()), ktype)
        }
        ExpressionPart::RecordLiteral(fields) => {
            let mut cells = BumpVec::with_capacity_in(fields.len(), scratch);
            cells.extend(
                fields
                    .iter()
                    .map(|(name, value)| (*name, digested(value, types, scratch))),
            );
            let ktype = record_type(
                types,
                scratch,
                cells.iter().map(|(name, (_, ktype))| (*name, *ktype)),
            );
            cells.sort_unstable_by_key(|(name, _)| name.symbol());
            let mut hasher = DigestHasher::new(Tag::Contents);
            hasher.count(cells.len());
            for (name, (cell, _)) in cells.iter() {
                hasher.feed(name.symbol()).digest(*cell);
            }
            (composite(Tag::Record, ktype, hasher.finished()), ktype)
        }
        _ => unreachable!("a lowerable part needs no dispatch"),
    }
}

/// Whether [`lower`] takes `part` whole.
fn lowers(part: &ExpressionPart<'_>) -> bool {
    match part {
        ExpressionPart::Literal(_) => true,
        ExpressionPart::ListLiteral(items) => items.iter().all(lowers),
        ExpressionPart::DictLiteral(pairs) => pairs.iter().all(|(key, value)| {
            let scalar = match key {
                ExpressionPart::Literal(KLiteral::Number(number)) => !number.is_nan(),
                ExpressionPart::Literal(KLiteral::String(_) | KLiteral::Boolean(_)) => true,
                _ => false,
            };
            scalar && lowers(value)
        }),
        ExpressionPart::RecordLiteral(fields) => fields.iter().all(|(_, value)| lowers(value)),
        ExpressionPart::Keyword(_)
        | ExpressionPart::Identifier(_)
        | ExpressionPart::Type(_)
        | ExpressionPart::Expression(_)
        | ExpressionPart::SigiledTypeExpr(_)
        | ExpressionPart::RecordType(_)
        | ExpressionPart::QuotedExpression(_)
        | ExpressionPart::MarkedName(..)
        | ExpressionPart::MarkedUse(..) => false,
    }
}

/// The value of a part [`lowers`] took.
fn lower<'cell, X: Knotted>(
    writer: Writer<'cell>,
    part: &ExpressionPart<'_>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Value<'cell, X> {
    match part {
        ExpressionPart::Literal(KLiteral::Number(number)) => Value::Number(*number),
        ExpressionPart::Literal(KLiteral::String(literal)) => text(writer, literal),
        ExpressionPart::Literal(KLiteral::Boolean(flag)) => Value::Bool(*flag),
        ExpressionPart::Literal(KLiteral::Null) => Value::Null,
        ExpressionPart::ListLiteral(items) => {
            let mut cells = BumpVec::with_capacity_in(items.len(), scratch);
            cells.extend(items.iter().map(|item| lower(writer, item, types, scratch)));
            Value::List(List::new(writer, cells.iter().copied(), types, scratch))
        }
        ExpressionPart::DictLiteral(pairs) => {
            let mut entries = BumpVec::with_capacity_in(pairs.len(), scratch);
            entries.extend(pairs.iter().map(|(key, value)| {
                // A string key borrows the literal where the parse left it; the dict door writes
                // every key into the region once.
                let key = match key {
                    ExpressionPart::Literal(KLiteral::String(literal)) => Key::str(literal),
                    ExpressionPart::Literal(KLiteral::Number(number)) => {
                        Key::number(*number).expect("a lowerable key is not NaN")
                    }
                    ExpressionPart::Literal(KLiteral::Boolean(flag)) => Key::bool(*flag),
                    _ => unreachable!("a lowerable dict key is a scalar literal"),
                };
                (key, lower(writer, value, types, scratch))
            }));
            Value::Dict(Dict::new(writer, &entries, types, scratch))
        }
        ExpressionPart::RecordLiteral(fields) => {
            let mut cells = BumpVec::with_capacity_in(fields.len(), scratch);
            cells.extend(
                fields
                    .iter()
                    .map(|(name, value)| (*name, lower(writer, value, types, scratch))),
            );
            Value::Record(Record::new(writer, &cells, types, scratch))
        }
        ExpressionPart::Keyword(_)
        | ExpressionPart::Identifier(_)
        | ExpressionPart::Type(_)
        | ExpressionPart::Expression(_)
        | ExpressionPart::SigiledTypeExpr(_)
        | ExpressionPart::RecordType(_)
        | ExpressionPart::QuotedExpression(_)
        | ExpressionPart::MarkedName(..)
        | ExpressionPart::MarkedUse(..) => unreachable!("a lowerable part needs no dispatch"),
    }
}
