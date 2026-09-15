//! The surface rendering of a value — what `PRINT` writes.
//!
//! **Circular values.** A render is two passes. The mark pass walks the value depth first, entering
//! each data node once, and records every node an edge reaches while that node is still being
//! entered: every cycle holds such a back edge, so every cycle holds a recorded target. The write
//! pass labels a target at its first occurrence, `@0 = …`, and writes every later occurrence as its
//! label, so it stops wherever a cycle closes; a node that is no target writes inline each time.
//! Labels count from zero in order of first appearance.

use std::fmt;

use crate::memory::{BumpAllocator, BumpVec};
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
        let mut marks = Marks {
            entering: BumpVec::new_in(scratch),
            entered: BumpVec::new_in(scratch),
            targets: BumpVec::new_in(scratch),
        };
        self.mark(&mut marks);
        let mut writer = Render {
            out,
            types,
            labels,
            scratch,
            targets: &marks.targets,
            labelled: BumpVec::new_in(scratch),
        };
        writer.value(self)
    }

    /// The mark pass: record every node reached again while it is being entered.
    fn mark(&self, marks: &mut Marks<'_, X>) {
        let Some((node, composite)) = self.composite() else {
            return;
        };
        if let Some(node) = node {
            if marks.entering.contains(&node) {
                if !marks.targets.contains(&node) {
                    marks.targets.push(node);
                }
                return;
            }
            if marks.entered.contains(&node) {
                return;
            }
            marks.entered.push(node);
            marks.entering.push(node);
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
        if node.is_some() {
            marks.entering.pop();
        }
    }
}

/// The mark pass's state: the nodes on the current path, every node entered, and the targets.
struct Marks<'x, X> {
    entering: BumpVec<'x, X>,
    entered: BumpVec<'x, X>,
    targets: BumpVec<'x, X>,
}

/// The write pass's state.
struct Render<'o, 'env, 'run, 'm, 'x, O, X> {
    out: &'o mut O,
    types: &'env TypeRegistry<'run>,
    labels: &'env LabelInterner,
    scratch: BumpAllocator<'x>,
    targets: &'m [X],
    /// Each target already written, in label order.
    labelled: BumpVec<'x, X>,
}

impl<O: fmt::Write, X: Knotted> Render<'_, '_, '_, '_, '_, O, X> {
    fn value(&mut self, value: &Value<'_, '_, X>) -> fmt::Result {
        let (types, labels) = (self.types, self.labels);
        if let Some(function) = value.as_callable() {
            return write!(
                self.out,
                "{}",
                display_name(function.ktype(), types, labels)
            );
        }
        if let Some((node, composite)) = value.composite() {
            if let Some(node) = node
                && self.targets.contains(&node)
            {
                if let Some(label) = self.labelled.iter().position(|seen| *seen == node) {
                    return write!(self.out, "@{label}");
                }
                write!(self.out, "@{} = ", self.labelled.len())?;
                self.labelled.push(node);
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
            | Value::Knotted(_) => unreachable!("a composite or a function wrote above"),
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
