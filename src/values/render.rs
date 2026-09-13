//! The surface rendering of a value — what `PRINT` writes.

use std::fmt;

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::LabelInterner;
use crate::type_lattice::{TypeRegistry, display_name};

use super::Value;

impl Value<'_, '_> {
    /// Render the value into `out`. A string writes its text bare, a dict key quoted; a list reads
    /// `[a, b]`, a dict `{k: v}` in key order, a record `{x = 1}` in field-name order; a tagged value
    /// reads as its type's name around its payload, a type as its name, and a quote as its body's
    /// surface. Sorting a record's fields by name is staged over `scratch`.
    pub fn render(
        &self,
        out: &mut impl fmt::Write,
        types: &TypeRegistry<'_>,
        labels: &LabelInterner,
        scratch: BumpAllocator<'_>,
    ) -> fmt::Result {
        match self {
            Value::Number(number) => write!(out, "{number}"),
            Value::Bool(flag) => write!(out, "{flag}"),
            Value::Null => out.write_str("null"),
            Value::Str(text) => out.write_str(text),
            Value::Expression(node) => write!(out, "{}", node.summary(labels)),
            Value::Type(value) => write!(out, "{}", display_name(value.handle(), types, labels)),
            Value::List(list) => {
                out.write_str("[")?;
                for (index, cell) in list.cells().iter().enumerate() {
                    if index > 0 {
                        out.write_str(", ")?;
                    }
                    cell.render(out, types, labels, scratch)?;
                }
                out.write_str("]")
            }
            Value::Dict(dict) => {
                out.write_str("{")?;
                for (index, (key, cell)) in dict.entries().enumerate() {
                    if index > 0 {
                        out.write_str(", ")?;
                    }
                    write!(out, "{key}: ")?;
                    cell.render(out, types, labels, scratch)?;
                }
                out.write_str("}")
            }
            Value::Record(record) => {
                // The layout is symbol order, which means nothing to a reader: the fields print in
                // the order of their names' text.
                let mut order = BumpVec::with_capacity_in(record.len(), scratch);
                order.extend(0..record.len());
                let names = record.names();
                order.sort_unstable_by(|left, right| {
                    labels.compare_texts(names[*left], names[*right])
                });
                out.write_str("{")?;
                for (index, at) in order.iter().enumerate() {
                    if index > 0 {
                        out.write_str(", ")?;
                    }
                    write!(out, "{} = ", labels.display(names[*at]))?;
                    record.cells()[*at].render(out, types, labels, scratch)?;
                }
                out.write_str("}")
            }
            Value::Tagged(tagged) => {
                write!(out, "{}(", display_name(tagged.ktype(), types, labels))?;
                tagged.payload().render(out, types, labels, scratch)?;
                out.write_str(")")
            }
        }
    }
}
