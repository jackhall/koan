//! The surface rendering of a value — what `PRINT` writes.
//!
//! A value is written at the type it is seen at, through [the door](super::surface). A plain value
//! is written in one walk. At the first data node the walk meets, a mark pass from
//! that node records every node a cycle returns to; the write then labels each at its first
//! occurrence, `@0 = …`, and writes `@0` after. A node is marked and labelled beside the type it is
//! seen at, since one node seen at two types shows two surfaces. See
//! [README.md § Equality and rendering](README.md#equality-and-rendering).
//!
//! Both walks run over explicit stacks in the scratch they are handed, so neither grows the call
//! stack with the value's depth: the mark pass keeps a frame per composite it is inside, and the
//! write keeps the [`Piece`]s still to be written, a composite writing its opener and pushing the
//! rest in reverse.

use std::fmt;

use crate::memory::{BumpAllocator, BumpBackedMap, BumpBackedSet, BumpVec, bump_set, bump_table};
use crate::symbols::{Symbol, SymbolInterner};
use crate::type_lattice::{KType, TypeRegistry, display_name};

use super::circular::Resolved;
use super::surface::Parts;
use super::{Key, Knotted, Seen, Surface, Value};

impl<'a, X: Knotted> Value<'a, X> {
    /// Render the value into `out`. A string writes its text bare, a dict key quoted; a list reads
    /// `[a, b]`, a dict `{k: v}` in key order, a record `{x = 1}` in field-name order; a tagged value
    /// reads as its type's name around its payload, a type as its name, a quote as its body's
    /// surface with its marks as written and never what they bind, a function, a module or a
    /// barrier as its type's name, and a knot's data node as the plain value of its kind, labelled
    /// where a cycle returns to it. Each part is written at the type it is seen at: a record shows
    /// the fields that type names, a tagged value its seen type's name around its payload at its
    /// representation. The marks, symbols, record field orders and both walks' stacks are staged
    /// over `scratch`.
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
        .value(Seen::of(*self))
    }
}

/// What the write has still to write, popped last pushed first: a value at the type it is seen
/// at, a fixed text, a dict key with its `: `, or a record field's name with its ` = `.
enum Piece<'a, X> {
    Value(Seen<'a, X>),
    Text(&'static str),
    Key(&'a Key<'a>),
    Field(Symbol),
}

/// A data node beside the type it is seen at: what the marks key, since one node seen at two
/// types shows two surfaces.
type Node<X> = (X, KType);

/// The mark pass's state: the nodes on the current path, every node entered, and the targets.
///
/// A mark pass runs from a node no earlier pass entered, and enters everything that node reaches.
/// A cycle through a fresh node and an entered one would have entered the fresh node too, so the
/// passes together make the marks one depth-first pass over the whole value would.
struct Marks<'x, X> {
    entering: BumpBackedSet<'x, Node<X>>,
    entered: BumpBackedSet<'x, Node<X>>,
    targets: BumpBackedSet<'x, Node<X>>,
}

/// The write's state.
struct Render<'o, 'env, 'run, 'x, O, X> {
    out: &'o mut O,
    types: &'env TypeRegistry<'run>,
    symbols: &'env SymbolInterner,
    scratch: BumpAllocator<'x>,
    marks: Marks<'x, X>,
    /// Each target already written, under its label.
    labelled: BumpBackedMap<'x, Node<X>, usize>,
}

impl<'x, O: fmt::Write, X: Knotted> Render<'_, '_, '_, 'x, O, X> {
    fn value<'a>(&mut self, root: Seen<'a, X>) -> fmt::Result {
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

    /// The mark pass from `node`, opened as `surface`, which no earlier pass entered: enter every
    /// node it reaches that no earlier pass entered, and record each node reached again while it is
    /// being entered.
    fn mark<'a>(&mut self, node: Node<X>, surface: Surface<'x, 'a, X>) {
        self.marks.entered.insert(node);
        self.marks.entering.insert(node);
        let (types, scratch) = (self.types, self.scratch);
        let mut frames = BumpVec::new_in(scratch);
        frames.push((Some(node), surface, 0));
        while let Some((_, surface, next)) = frames.last_mut() {
            if *next < surface.len() {
                let child = surface.child(*next, types, scratch);
                *next += 1;
                if let Some((node, surface)) = self.entered(child) {
                    frames.push((node, surface, 0));
                }
            } else if let Some((Some(node), _, _)) = frames.pop() {
                self.marks.entering.remove(&node);
            }
        }
    }

    /// `seen`'s surface when the mark pass descends into it, beside the data node it is: a node
    /// already on the path is recorded as a target instead, and one entered before is skipped.
    fn entered<'a>(&mut self, seen: Seen<'a, X>) -> Option<(Option<Node<X>>, Surface<'x, 'a, X>)> {
        let surface = seen.surface(self.types, self.scratch)?;
        let node = surface.node().map(|node| (node, surface.ktype()));
        if let Some(node) = node {
            let marks = &mut self.marks;
            if marks.entering.contains(&node) {
                marks.targets.insert(node);
                return None;
            }
            if !marks.entered.insert(node) {
                return None;
            }
            marks.entering.insert(node);
        }
        Some((node, surface))
    }

    /// Write `seen` up to its contents: a leaf, an opaque member or a label whole, a surface's
    /// opener, with the rest of the surface pushed onto `pieces`.
    fn one<'a>(
        &mut self,
        seen: Seen<'a, X>,
        pieces: &mut BumpVec<'_, Piece<'a, X>>,
    ) -> fmt::Result {
        let (types, symbols) = (self.types, self.symbols);
        let value = seen.value();
        if let Value::Knotted(member) = value {
            match member.resolve() {
                Resolved::Code(code) => return write!(self.out, "{}", code.body.summary(symbols)),
                Resolved::Function { .. } | Resolved::Module | Resolved::Barrier => {
                    return write!(self.out, "{}", display_name(member.ktype(), types, symbols));
                }
                Resolved::Circular(_) => {}
            }
        }
        if let Some(surface) = seen.surface(types, self.scratch) {
            if let Some(node) = surface.node() {
                let node = (node, surface.ktype());
                if !self.marks.entered.contains(&node) {
                    self.mark(node, surface);
                }
                if self.marks.targets.contains(&node) {
                    if let Some(label) = self.labelled.get(&node) {
                        return write!(self.out, "@{label}");
                    }
                    write!(self.out, "@{} = ", self.labelled.len())?;
                    self.labelled.insert(node, self.labelled.len());
                }
            }
            return self.open(surface, pieces);
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
            | Value::Knotted(_) => unreachable!("a surface or an opaque member wrote above"),
        }
    }

    /// Write `surface`'s opener and push the rest of it, in reverse. A record's layout is symbol
    /// order, which means nothing to a reader: its fields print in the order of their names' text.
    fn open<'a>(
        &mut self,
        surface: Surface<'_, 'a, X>,
        pieces: &mut BumpVec<'_, Piece<'a, X>>,
    ) -> fmt::Result {
        let (types, symbols, scratch) = (self.types, self.symbols, self.scratch);
        let child = |at| Piece::Value(surface.child(at, types, scratch));
        match surface.parts() {
            Parts::List { .. } => {
                self.out.write_str("[")?;
                pieces.push(Piece::Text("]"));
                for index in (0..surface.len()).rev() {
                    pieces.push(child(index));
                    if index > 0 {
                        pieces.push(Piece::Text(", "));
                    }
                }
            }
            Parts::Dict { .. } => {
                self.out.write_str("{")?;
                pieces.push(Piece::Text("}"));
                for index in (0..surface.len()).rev() {
                    pieces.push(child(index));
                    pieces.push(Piece::Key(surface.key(index)));
                    if index > 0 {
                        pieces.push(Piece::Text(", "));
                    }
                }
            }
            Parts::Record { .. } => {
                let mut order = BumpVec::with_capacity_in(surface.len(), scratch);
                order.extend(0..surface.len());
                order.sort_unstable_by(|left, right| {
                    symbols.compare_texts(surface.name(*left), surface.name(*right))
                });
                self.out.write_str("{")?;
                pieces.push(Piece::Text("}"));
                for (index, at) in order.iter().enumerate().rev() {
                    pieces.push(child(*at));
                    pieces.push(Piece::Field(surface.name(*at)));
                    if index > 0 {
                        pieces.push(Piece::Text(", "));
                    }
                }
            }
            Parts::Tagged { .. } => {
                write!(
                    self.out,
                    "{}(",
                    display_name(surface.ktype(), types, symbols)
                )?;
                pieces.push(Piece::Text(")"));
                pieces.push(child(0));
            }
        }
        Ok(())
    }
}
