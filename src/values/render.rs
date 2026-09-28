//! The surface rendering of a value — what `PRINT` writes.
//!
//! A plain value is written in one walk. At the first data node the walk meets, a mark pass from
//! that node records every node a cycle returns to; the write then labels each at its first
//! occurrence, `@0 = …`, and writes `@0` after. See
//! [README.md § Equality and rendering](README.md#equality-and-rendering).
//!
//! Both walks run over explicit stacks in the scratch they are handed, so neither grows the call
//! stack with the value's depth: the mark pass keeps a frame per composite it is inside, and the
//! write keeps the [`Piece`]s still to be written, a composite writing its opener and pushing the
//! rest in reverse.

use std::fmt;

use crate::memory::{BumpAllocator, BumpBackedMap, BumpBackedSet, BumpVec, bump_set, bump_table};
use crate::symbols::{Symbol, SymbolInterner};
use crate::type_lattice::{TypeRegistry, display_name};

use super::circular::{Composite, Resolved};
use super::{Key, Knotted, Value};

impl<'a, X: Knotted> Value<'a, X> {
    /// Render the value into `out`. A string writes its text bare, a dict key quoted; a list reads
    /// `[a, b]`, a dict `{k: v}` in key order, a record `{x = 1}` in field-name order; a tagged value
    /// reads as its type's name around its payload, a type as its name, a quote as its body's
    /// surface with its marks as written and never what they bind, a function, a module or a
    /// barrier as its type's name, and a knot's data node as the plain value of its kind, labelled
    /// where a cycle returns to it. The marks, symbols, record field orders and both walks' stacks
    /// are staged over `scratch`.
    pub fn render(
        &self,
        out: &mut impl fmt::Write,
        types: &TypeRegistry<'_>,
        symbols: &SymbolInterner,
        scratch: BumpAllocator<'_>,
    ) -> fmt::Result {
        Render {
            out,
            types,
            symbols,
            scratch,
            marks: Marks {
                entering: bump_set(scratch),
                entered: bump_set(scratch),
                targets: bump_set(scratch),
            },
            labelled: bump_table(scratch),
        }
        .value(*self)
    }

    /// The mark pass from this value: enter every node it reaches that no earlier pass entered, and
    /// record each node reached again while it is being entered.
    fn mark(&self, marks: &mut Marks<'_, X>, scratch: BumpAllocator<'_>) {
        let Some((node, composite)) = self.entered(marks) else {
            return;
        };
        let mut frames = BumpVec::new_in(scratch);
        frames.push((node, composite, 0));
        while let Some((_, composite, next)) = frames.last_mut() {
            if *next < composite.len() {
                let child = composite.child(*next);
                *next += 1;
                if let Some((node, composite)) = child.entered(marks) {
                    frames.push((node, composite, 0));
                }
            } else if let Some((Some(node), _, _)) = frames.pop() {
                marks.entering.remove(&node);
            }
        }
    }

    /// This value's composite when the mark pass descends into it, beside the data node it is: a
    /// node already on the path is recorded as a target instead, and one entered before is skipped.
    fn entered(&self, marks: &mut Marks<'_, X>) -> Option<(Option<X>, Composite<'a, X>)> {
        let (node, composite) = self.composite()?;
        if let Some(node) = node {
            if marks.entering.contains(&node) {
                marks.targets.insert(node);
                return None;
            }
            if !marks.entered.insert(node) {
                return None;
            }
            marks.entering.insert(node);
        }
        Some((node, composite))
    }
}

/// What the write has still to write, popped last pushed first: a value, a fixed text, a dict key
/// with its `: `, or a record field's name with its ` = `.
enum Piece<'a, X> {
    Value(Value<'a, X>),
    Text(&'static str),
    Key(&'a Key<'a>),
    Field(Symbol),
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
    symbols: &'env SymbolInterner,
    scratch: BumpAllocator<'x>,
    marks: Marks<'x, X>,
    /// Each target already written, under its label.
    labelled: BumpBackedMap<'x, X, usize>,
}

impl<O: fmt::Write, X: Knotted> Render<'_, '_, '_, '_, O, X> {
    fn value<'a>(&mut self, root: Value<'a, X>) -> fmt::Result {
        let mut pieces = BumpVec::new_in(self.scratch);
        pieces.push(Piece::Value(root));
        while let Some(piece) = pieces.pop() {
            match piece {
                Piece::Value(value) => self.one(value, &mut pieces)?,
                Piece::Text(text) => self.out.write_str(text)?,
                Piece::Key(key) => write!(self.out, "{key}: ")?,
                Piece::Field(name) => write!(self.out, "{} = ", self.symbols.display(name))?,
            }
        }
        Ok(())
    }

    /// Write `value` up to its contents: a leaf, an opaque member or a label whole, a composite's
    /// opener, with the rest of the composite pushed onto `pieces`.
    fn one<'a>(
        &mut self,
        value: Value<'a, X>,
        pieces: &mut BumpVec<'_, Piece<'a, X>>,
    ) -> fmt::Result {
        let (types, symbols) = (self.types, self.symbols);
        if let Value::Knotted(member) = value {
            match member.resolve() {
                Resolved::Code(code) => return write!(self.out, "{}", code.body.summary(symbols)),
                Resolved::Function { .. } | Resolved::Module | Resolved::Barrier => {
                    return write!(self.out, "{}", display_name(member.ktype(), types, symbols));
                }
                Resolved::Circular(_) => {}
            }
        }
        if let Some((node, composite)) = value.composite() {
            if let Some(node) = node {
                if !self.marks.entered.contains(&node) {
                    value.mark(&mut self.marks, self.scratch);
                }
                if self.marks.targets.contains(&node) {
                    if let Some(label) = self.labelled.get(&node) {
                        return write!(self.out, "@{label}");
                    }
                    write!(self.out, "@{} = ", self.labelled.len())?;
                    self.labelled.insert(node, self.labelled.len());
                }
            }
            return self.open(composite, pieces);
        }
        match value {
            Value::Number(number) => write!(self.out, "{number}"),
            Value::Bool(flag) => write!(self.out, "{flag}"),
            Value::Null => self.out.write_str("null"),
            Value::Str(text) => self.out.write_str(text),
            Value::Type(value) => {
                write!(self.out, "{}", display_name(value.handle(), types, symbols))
            }
            Value::List(_)
            | Value::Dict(_)
            | Value::Record(_)
            | Value::Tagged(_)
            | Value::Knotted(_) => unreachable!("a composite or an opaque member wrote above"),
        }
    }

    /// Write `composite`'s opener and push the rest of it, in reverse. A record's layout is symbol
    /// order, which means nothing to a reader: its fields print in the order of their names' text.
    fn open<'a>(
        &mut self,
        composite: Composite<'a, X>,
        pieces: &mut BumpVec<'_, Piece<'a, X>>,
    ) -> fmt::Result {
        let (types, symbols) = (self.types, self.symbols);
        match composite {
            Composite::List { cells, .. } => {
                self.out.write_str("[")?;
                pieces.push(Piece::Text("]"));
                for index in (0..cells.len()).rev() {
                    pieces.push(Piece::Value(cells.get(index)));
                    if index > 0 {
                        pieces.push(Piece::Text(", "));
                    }
                }
            }
            Composite::Dict { keys, cells, .. } => {
                self.out.write_str("{")?;
                pieces.push(Piece::Text("}"));
                for index in (0..keys.len()).rev() {
                    pieces.push(Piece::Value(cells.get(index)));
                    pieces.push(Piece::Key(&keys[index]));
                    if index > 0 {
                        pieces.push(Piece::Text(", "));
                    }
                }
            }
            Composite::Record { names, cells, .. } => {
                let mut order = BumpVec::with_capacity_in(names.len(), self.scratch);
                order.extend(0..names.len());
                order.sort_unstable_by(|left, right| {
                    symbols.compare_texts(names[*left], names[*right])
                });
                self.out.write_str("{")?;
                pieces.push(Piece::Text("}"));
                for (index, at) in order.iter().enumerate().rev() {
                    pieces.push(Piece::Value(cells.get(*at)));
                    pieces.push(Piece::Field(names[*at]));
                    if index > 0 {
                        pieces.push(Piece::Text(", "));
                    }
                }
            }
            Composite::Tagged { ktype, payload } => {
                write!(self.out, "{}(", display_name(ktype, types, symbols))?;
                pieces.push(Piece::Text(")"));
                pieces.push(Piece::Value(payload));
            }
        }
        Ok(())
    }
}
