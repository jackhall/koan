//! The surface rendering of a value — what `PRINT` writes.
//!
//! A plain value is written in one walk. At the first data node the walk meets, a mark pass from
//! that node records every node a cycle returns to; the write then labels each at its first
//! occurrence, `@0 = …`, and writes `@0` after. See
//! [README.md § Equality and rendering](README.md#equality-and-rendering).

use std::fmt;

use crate::memory::{BumpAllocator, BumpBackedMap, BumpBackedSet, BumpVec, bump_set, bump_table};
use crate::parse::LabelInterner;
use crate::type_lattice::{TypeRegistry, display_name};

use super::circular::{Cells, Composite};
use super::{Knotted, Value};

impl<X: Knotted> Value<'_, '_, X> {
    /// Render the value into `out`. A string writes its text bare, a dict key quoted; a list reads
    /// `[a, b]`, a dict `{k: v}` in key order, a record `{x = 1}` in field-name order; a tagged value
    /// reads as its type's name around its payload, a type as its name, a quote as its body's
    /// surface, a function as its type's name, and a knot's data node as the plain value of its
    /// kind, labelled where a cycle returns to it. The marks, labels and record field orders are
    /// staged over `scratch`.
    pub fn render(
        &self,
        out: &mut impl fmt::Write,
        types: &TypeRegistry<'_>,
        labels: &LabelInterner,
        scratch: BumpAllocator<'_>,
    ) -> fmt::Result {
        Render {
            out,
            types,
            labels,
            scratch,
            marks: Marks {
                entering: bump_set(scratch),
                entered: bump_set(scratch),
                targets: bump_set(scratch),
            },
            labelled: bump_table(scratch),
        }
        .value(self)
    }

    /// The mark pass from this value: enter every node it reaches that no earlier pass entered, and
    /// record each node reached again while it is being entered.
    fn mark(&self, marks: &mut Marks<'_, X>) {
        let Some((node, composite)) = self.composite() else {
            return;
        };
        if let Some(node) = node {
            if marks.entering.contains(&node) {
                marks.targets.insert(node);
                return;
            }
            if !marks.entered.insert(node) {
                return;
            }
            marks.entering.insert(node);
        }
        match composite {
            Composite::List { cells, .. }
            | Composite::Dict { cells, .. }
            | Composite::Record { cells, .. } => {
                for cell in cells.iter() {
                    cell.mark(marks);
                }
            }
            Composite::Tagged { payload, .. } => payload.mark(marks),
        }
        if let Some(node) = node {
            marks.entering.remove(&node);
        }
    }
}

/// The mark pass's state: the nodes on the current path, every node entered, and the targets.
///
/// A mark pass runs from a node no earlier pass entered, and enters everything that node reaches.
/// A cycle through a fresh node and an entered one would have entered the fresh node too, so the
/// passes together make the marks one depth-first pass over the whole value would.
struct Marks<'x, X> {
    entering: BumpBackedSet<'x, X>,
    entered: BumpBackedSet<'x, X>,
    targets: BumpBackedSet<'x, X>,
}

/// The write's state.
struct Render<'o, 'env, 'run, 'x, O, X> {
    out: &'o mut O,
    types: &'env TypeRegistry<'run>,
    labels: &'env LabelInterner,
    scratch: BumpAllocator<'x>,
    marks: Marks<'x, X>,
    /// Each target already written, under its label.
    labelled: BumpBackedMap<'x, X, usize>,
}

impl<O: fmt::Write, X: Knotted> Render<'_, '_, '_, '_, O, X> {
    fn value(&mut self, value: &Value<'_, '_, X>) -> fmt::Result {
        let (types, labels) = (self.types, self.labels);
        if let Some(opaque) = value.as_opaque() {
            return write!(self.out, "{}", display_name(opaque.ktype(), types, labels));
        }
        if let Some((node, composite)) = value.composite() {
            if let Some(node) = node {
                if !self.marks.entered.contains(&node) {
                    value.mark(&mut self.marks);
                }
                if self.marks.targets.contains(&node) {
                    if let Some(label) = self.labelled.get(&node) {
                        return write!(self.out, "@{label}");
                    }
                    write!(self.out, "@{} = ", self.labelled.len())?;
                    self.labelled.insert(node, self.labelled.len());
                }
            }
            return self.composite(composite);
        }
        match value {
            Value::Number(number) => write!(self.out, "{number}"),
            Value::Bool(flag) => write!(self.out, "{flag}"),
            Value::Null => self.out.write_str("null"),
            Value::Str(text) => self.out.write_str(text),
            Value::Expression(node) => write!(self.out, "{}", node.summary(labels)),
            Value::Type(value) => {
                write!(self.out, "{}", display_name(value.handle(), types, labels))
            }
            Value::List(_)
            | Value::Dict(_)
            | Value::Record(_)
            | Value::Tagged(_)
            | Value::Knotted(_) => unreachable!("a composite or an opaque member wrote above"),
        }
    }

    fn composite(&mut self, composite: Composite<'_, X>) -> fmt::Result {
        let (types, labels) = (self.types, self.labels);
        match composite {
            Composite::List { cells, .. } => {
                self.out.write_str("[")?;
                for (index, cell) in cells.iter().enumerate() {
                    if index > 0 {
                        self.out.write_str(", ")?;
                    }
                    self.value(&cell)?;
                }
                self.out.write_str("]")
            }
            Composite::Dict { keys, cells, .. } => {
                self.out.write_str("{")?;
                for (index, key) in keys.iter().enumerate() {
                    if index > 0 {
                        self.out.write_str(", ")?;
                    }
                    write!(self.out, "{key}: ")?;
                    self.value(&cells.get(index))?;
                }
                self.out.write_str("}")
            }
            Composite::Record { names, cells, .. } => self.record(names, cells),
            Composite::Tagged { ktype, payload } => {
                write!(self.out, "{}(", display_name(ktype, types, labels))?;
                self.value(&payload)?;
                self.out.write_str(")")
            }
        }
    }

    /// The layout is symbol order, which means nothing to a reader: the fields print in the order
    /// of their names' text.
    fn record(&mut self, names: &[crate::parse::Symbol], cells: Cells<'_, X>) -> fmt::Result {
        let labels = self.labels;
        let mut order = BumpVec::with_capacity_in(names.len(), self.scratch);
        order.extend(0..names.len());
        order.sort_unstable_by(|left, right| labels.compare_texts(names[*left], names[*right]));
        self.out.write_str("{")?;
        for (index, at) in order.iter().enumerate() {
            if index > 0 {
                self.out.write_str(", ")?;
            }
            write!(self.out, "{} = ", labels.display(names[*at]))?;
            self.value(&cells.get(*at))?;
        }
        self.out.write_str("}")
    }
}
