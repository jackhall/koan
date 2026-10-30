//! **Content digests**: what identifies a value for the life of the loaded program, by what it
//! holds and never by where it sits.
//!
//! A [`ContentDigest`] is the low 128 bits of a BLAKE3 hash. The recipe is one rule throughout: a
//! digest is a domain [`Tag`], then the value's own scalar payload, then its parts' digests.
//!
//! A value digests as what [the door](super::surface) shows of it at the type it is seen at,
//! exactly as the [deep copy](super::crossing) lays it down, so a copy digests as its source and a
//! cell its type hides counts for nothing:
//!
//! - A scalar digests from its payload: a number from its bits, a bool, `null`, a string
//!   length-prefixed, a type value from its handle's bits.
//! - A list, dict, record and tagged value, and a data node seen at a type other than its memo,
//!   digest as its kind's tag, its seen type's handle and its **contents**: each part the door
//!   shows, at its type there — a list its elements in order, a dict each key's digest then each
//!   cell's, in key order, a record each shown field's name and cell in symbol order, so field
//!   order is blind, and a tagged value its payload at its representation.
//! - A knot member seen at its own memo digests as its knot's, beside its index there: a knot
//!   digests its node count and each node's content in index order — a data node read through the
//!   door at its memo, an edge to a sibling hashed as its index — so values that reach one another
//!   digest as the one knot they are tied into. Its layer decides what a node's content is: the
//!   member [lists](super::Knotted::held) the values its knot holds and
//!   [hashes](super::Knotted::digest_held) the knot over their digests.
//!
//! Each recipe over digests is a function of its own — a literal's, a type value's, a knot's, a
//! member's, a closure's, a body-born module's and a view's — which the run's walk calls and which
//! the load calls over the digests it computes from the program's text, so the two cannot key one
//! content two ways.
//!
//! The seen type is part of the recipe, so a retype changes the digest. Inside a knot each node is
//! digested at its own memo, so a cell a tagged node's representation hides still counts there, as
//! a copy of the knot keeps it. That over-distinction is sound: a digest keys what is equal by
//! content, and keying two equal things apart costs only a key.
//!
//! A digest is computed on demand — by a view, or by a settlement's key — and kept only by a
//! module, as its content, and by a slot the load marks digested, beside its value; a value no
//! demand reaches never pays for one. A demand walks what the value reaches through one
//! [`Digests`] memo over the demand's scratch, so a part shared many times over, and a knot met
//! through any of its members, is digested once per demand. The walk runs over an explicit stack,
//! as the copy does: a composite is a frame over the parts its surface shows, and a knot member a
//! frame over the values its knot holds, so neither a value's depth nor a chain of knots grows the
//! call stack.

use std::hash::{Hash, Hasher};

use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, Edge, bump_table};
use crate::parse::{ExpressionPart, KExpression, KLiteral, ListItem};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{DeclaredType, KType};

use super::surface::Parts;
use super::{Chain, Knotted, Nothing, Seen, Surface, Text, Value};

/// A value's content identity: the low 128 bits of a BLAKE3 hash of its content, held as bytes so a
/// node storing one keeps a word's alignment.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ContentDigest([u8; 16]);

impl ContentDigest {
    /// A placeholder for a digest not yet computed.
    pub const NONE: ContentDigest = ContentDigest([0; 16]);

    /// The digest's bits, for an identity keyed on content outside `values` — a carrier's.
    pub fn bits(self) -> u128 {
        u128::from_le_bytes(self.0)
    }
}

/// The domain tag every digest begins with, one per digestible shape, so no two shapes share a
/// digest even with identical payloads. Never reorder or reuse one. The scalar tags also open a
/// scalar literal inside a body's code digest, which hashes a literal as syntax.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Tag {
    Number = 0x01,
    Bool = 0x02,
    Null = 0x03,
    Str = 0x04,
    Type = 0x05,
    List = 0x06,
    Dict = 0x07,
    Record = 0x08,
    Tagged = 0x09,
    /// A run of digests: a container's contents.
    Contents = 0x0A,
    /// A knot: its node count and each node's content in index order.
    Knot = 0x10,
    /// One member of a knot: the knot's digest and the member's index.
    Member = 0x11,
    /// An edge inside a knot, by index.
    Edge = 0x12,
    Builtin = 0x13,
    Function = 0x14,
    Module = 0x15,
    View = 0x16,
    Barrier = 0x17,
    Code = 0x18,
    Data = 0x19,
    /// A knot member seen at a type other than its own: the type, then the member's digest.
    Retyped = 0x1A,
    /// A body's code, digested as resolved, where its shape is built.
    Shape = 0x20,
    /// A name read in a body's own activation or a block it hops out of.
    Local = 0x21,
    /// A name bound at the program's top level.
    Top = 0x22,
    /// A name a body captures from outside it, short of the top level.
    Capture = 0x23,
    /// A name a quote's code leaves open, or one an `EVAL` offers.
    Open = 0x24,
    Keyword = 0x25,
    /// A keyworded use and its candidates.
    Use = 0x26,
    /// A spread candidate: a list of functions read at a coordinate.
    Spread = 0x27,
    /// A part that is a nested body, by its code digest.
    Nested = 0x28,
    /// A container literal, or a group of parts.
    Group = 0x29,
    /// A type part the load typed, by its handle.
    Typed = 0x2A,
    /// A type part the load left unknown, by its syntax.
    Untyped = 0x2B,
    /// A mark on a name or a use, by which mark.
    Mark = 0x2C,
    /// A view's operator: `:|`, or the re-view a nested module takes at an opaque boundary.
    Opaque = 0x2E,
    Reviewed = 0x2F,
    /// What the load cannot see of a module's content, keyed on its binder: a load-time key, never
    /// a value's digest.
    Leaf = 0x30,
    /// A settlement's key: a body's segment index and the argument digests it is keyed on.
    Settlement = 0x31,
    /// Code as lowered syntax: what code equality compares, spans and caches left out.
    Syntax = 0x32,
    /// A `$..` splice of a quote's syntax, by the name it spreads.
    Splice = 0x33,
    /// A use a `USING` filled: its scope, the keys it fills, and the use.
    FilledUse = 0x34,
    /// A composition binding a callable's parameters: its code's syntax digest, then the names.
    Composition = 0x35,
}

/// The one hasher every digest is computed through: a tag first, then payload, then each part's
/// digest, every integer fed little-endian. It is a [`Hasher`] too, so a lattice handle or a symbol
/// feeds its own bits through its `Hash`.
pub struct DigestHasher(blake3::Hasher);

impl DigestHasher {
    /// A hasher for a digest of the shape `tag`.
    pub fn new(tag: Tag) -> DigestHasher {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&[tag as u8]);
        DigestHasher(hasher)
    }

    /// Feed a tag inside a run: the shape of the part that follows.
    pub fn tag(&mut self, tag: Tag) -> &mut Self {
        self.0.update(&[tag as u8]);
        self
    }

    /// Feed `value`'s bits through its `Hash`: a lattice handle's digest, a symbol's bits.
    pub fn feed(&mut self, value: impl Hash) -> &mut Self {
        value.hash(self);
        self
    }

    /// Feed a count or an index.
    pub fn count(&mut self, count: usize) -> &mut Self {
        self.0.update(&(count as u64).to_le_bytes());
        self
    }

    /// Feed a part's digest.
    pub fn digest(&mut self, digest: ContentDigest) -> &mut Self {
        self.0.update(&digest.0);
        self
    }

    /// Feed a run of bytes, length-prefixed.
    pub fn text(&mut self, text: &[u8]) -> &mut Self {
        self.count(text.len());
        self.0.update(text);
        self
    }

    /// The digest of everything fed.
    pub fn finished(&self) -> ContentDigest {
        let bytes = self.0.finalize();
        let mut low = [0u8; 16];
        low.copy_from_slice(&bytes.as_bytes()[..16]);
        ContentDigest(low)
    }
}

impl Hasher for DigestHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    /// The low 64 bits of the digest, for a reader that asks a `Hasher` for one.
    fn finish(&self) -> u64 {
        self.finished().bits() as u64
    }
}

/// The contents a run of part digests makes: their count, then each in order.
fn contents(parts: impl ExactSizeIterator<Item = ContentDigest>) -> ContentDigest {
    let mut hasher = DigestHasher::new(Tag::Contents);
    hasher.count(parts.len());
    for part in parts {
        hasher.digest(part);
    }
    hasher.finished()
}

/// A composite's digest: its kind's tag, its seen type's handle, and the contents it shows there.
fn composite(tag: Tag, ktype: KType, contents: ContentDigest) -> ContentDigest {
    DigestHasher::new(tag)
        .feed(ktype)
        .digest(contents)
        .finished()
}

/// One demand's memo, beside the registry its walk reads types in and the scratch it stages over:
/// the digest of each composite a walk has already met, keyed by the shape it digests as, the
/// address of the resident it sits in, and the type it is seen at, since one node seen at two types
/// shows two surfaces; and the digest of each knot it has met, keyed by the knot's root member.
/// Nothing a walk reads is freed or moved while it runs, so an address names one part of a shape
/// for the walk's life, and the shape keeps two kinds of resident that could share an address
/// apart.
pub struct Digests<'a, 'run, 'p, 'reg, X> {
    types: &'a Chain<'run, 'p, 'reg, 'a>,
    scratch: BumpAllocator<'a>,
    composites: BumpBackedMap<'a, MemoKey, ContentDigest>,
    knots: BumpBackedMap<'a, X, ContentDigest>,
}

/// What a composite's memo entry is keyed on: the shape's tag, the resident's address, the seen
/// type.
type MemoKey = (u8, usize, DeclaredType<KType>);

/// One pending frame of a digest walk: how many of its parts have been started, and where its
/// finished parts begin on the walk's `done` stack.
#[derive(Clone, Copy)]
enum Frame<'x, 'cell, X> {
    /// A list, dict, record or tagged value, opened at the type it is seen at, beside its memo key.
    Composite {
        surface: Surface<'x, 'cell, X>,
        key: MemoKey,
        next: usize,
        base: usize,
    },
    /// A knot member seen at its own memo whose knot this demand has not digested: the values its
    /// knot holds sit on the walk's `held` stack from `held` to `end`.
    Knot {
        member: X,
        held: usize,
        end: usize,
        next: usize,
        base: usize,
    },
    /// A knot member seen at `carried`, a type other than its own: its one part is the bare
    /// member at its own type.
    Retyped {
        carried: DeclaredType<KType>,
        member: X,
        next: usize,
        base: usize,
    },
}

/// One walk's stacks: its pending frames, the finished digests of their parts, and the values each
/// pending knot holds, as its member listed them.
struct Stacks<'a, 'cell, X> {
    frames: BumpVec<'a, Frame<'a, 'cell, X>>,
    done: BumpVec<'a, ContentDigest>,
    held: BumpVec<'a, Seen<'cell, X>>,
}

impl<'a, 'run, 'p, 'reg, X: Knotted> Digests<'a, 'run, 'p, 'reg, X> {
    /// A demand's memo, reading types in `types` and staging its walks and its tables over
    /// `scratch`.
    pub fn new(types: &'a Chain<'run, 'p, 'reg, '_>, scratch: BumpAllocator<'a>) -> Self {
        #[cfg(test)]
        DIGEST_DEMANDS.with(|count| count.set(count.get() + 1));
        Digests {
            types,
            scratch,
            composites: bump_table(scratch),
            knots: bump_table(scratch),
        }
    }

    /// The digest of `top`, read through the door at the type it is seen at. A composite is a
    /// frame over the parts its surface shows, and a knot member seen at its own memo a frame over
    /// the values its knot [holds](Knotted::held), so neither a value's depth nor a chain of knots
    /// grows the call stack.
    pub(super) fn seen<'cell>(&mut self, top: Seen<'cell, X>) -> ContentDigest
    where
        X: 'cell,
    {
        let mut stacks = Stacks {
            frames: BumpVec::new_in(self.scratch),
            done: BumpVec::new_in(self.scratch),
            held: BumpVec::new_in(self.scratch),
        };
        if let Some(leaf) = self.start(top, &mut stacks) {
            return leaf;
        }
        while let Some(&frame) = stacks.frames.last() {
            match self.next_part(frame, &stacks) {
                Some(part) => {
                    if let Some(
                        Frame::Composite { next, .. }
                        | Frame::Knot { next, .. }
                        | Frame::Retyped { next, .. },
                    ) = stacks.frames.last_mut()
                    {
                        *next += 1;
                    }
                    if let Some(leaf) = self.start(part, &mut stacks) {
                        stacks.done.push(leaf);
                    }
                }
                None => {
                    stacks.frames.pop();
                    let finished = self.finish(frame, &mut stacks);
                    if stacks.frames.is_empty() {
                        return finished;
                    }
                    stacks.done.push(finished);
                }
            }
        }
        unreachable!("a started composite or knot pushes a frame")
    }

    /// Begin digesting `seen`: a leaf, or a composite or knot this demand already digested, is
    /// finished at once; any other composite or knot member pushes its frame. A knot member seen at
    /// a type other than its own pushes a frame over the bare member, which digests beside that
    /// type.
    fn start<'cell>(
        &mut self,
        seen: Seen<'cell, X>,
        stacks: &mut Stacks<'a, 'cell, X>,
    ) -> Option<ContentDigest>
    where
        X: 'cell,
    {
        let base = stacks.done.len();
        let value = seen.value();
        match value {
            Value::List(_) | Value::Dict(_) | Value::Record(_) | Value::Tagged(_) => {}
            Value::Knotted(_) | Value::Retyped(_) => {
                let member = value.knot_member().expect("a knot member");
                if member.ktype() != seen.ktype() {
                    #[cfg(test)]
                    DIGEST_VISITS.with(|count| count.set(count.get() + 1));
                    stacks.frames.push(Frame::Retyped {
                        carried: seen.ktype(),
                        member,
                        next: 0,
                        base,
                    });
                    return None;
                }
                if let Some(knot) = self.knots.get(&member.root()) {
                    return Some(beside_index(*knot, member));
                }
                #[cfg(test)]
                DIGEST_VISITS.with(|count| count.set(count.get() + 1));
                let held = stacks.held.len();
                member.held(self.types, self.scratch, &mut |part| stacks.held.push(part));
                stacks.frames.push(Frame::Knot {
                    member,
                    held,
                    end: stacks.held.len(),
                    next: 0,
                    base,
                });
                return None;
            }
            leaf => {
                #[cfg(test)]
                DIGEST_VISITS.with(|count| count.set(count.get() + 1));
                return Some(scalar(leaf));
            }
        }
        let (tag, address) = resident(value);
        let key = (tag as u8, address, seen.ktype());
        if let Some(digest) = self.composites.get(&key) {
            return Some(*digest);
        }
        #[cfg(test)]
        DIGEST_VISITS.with(|count| count.set(count.get() + 1));
        let surface = seen
            .surface(self.types, self.scratch)
            .expect("a container or a tagged value opens");
        stacks.frames.push(Frame::Composite {
            surface,
            key,
            next: 0,
            base,
        });
        None
    }

    /// The next part `frame` has not started, if any.
    fn next_part<'cell>(
        &self,
        frame: Frame<'a, 'cell, X>,
        stacks: &Stacks<'a, 'cell, X>,
    ) -> Option<Seen<'cell, X>>
    where
        X: 'cell,
    {
        match frame {
            Frame::Composite { surface, next, .. } => {
                (next < surface.len()).then(|| surface.child(next, self.types, self.scratch))
            }
            Frame::Knot {
                held, end, next, ..
            } => (held + next < end).then(|| stacks.held[held + next]),
            Frame::Retyped { member, next, .. } => {
                (next == 0).then(|| Seen::of(Value::Knotted(member)))
            }
        }
    }

    /// `frame`'s digest over its finished parts, memoized, and its parts popped: a composite's
    /// kind, seen type and contents; a knot member's knot, from its member's
    /// [`digest_held`](Knotted::digest_held), beside its index there; a retyped member's type,
    /// over its bare member's digest.
    fn finish<'cell>(
        &mut self,
        frame: Frame<'a, 'cell, X>,
        stacks: &mut Stacks<'a, 'cell, X>,
    ) -> ContentDigest
    where
        X: 'cell,
    {
        let (base, finished) = match frame {
            Frame::Composite {
                surface, key, base, ..
            } => {
                let digest = composed(&surface, &stacks.done[base..]);
                self.composites.insert(key, digest);
                (base, digest)
            }
            Frame::Knot {
                member, held, base, ..
            } => {
                let mut parts = stacks.done[base..].iter().copied();
                let knot = member.digest_held(self.types, self.scratch, &mut || {
                    parts
                        .next()
                        .expect("digest_held asks for each held value once")
                });
                debug_assert!(
                    parts.next().is_none(),
                    "digest_held asks for every held value"
                );
                self.knots.insert(member.root(), knot);
                stacks.held.truncate(held);
                (base, beside_index(knot, member))
            }
            Frame::Retyped { carried, base, .. } => {
                (base, retyped_content(carried, stacks.done[base]))
            }
        };
        stacks.done.truncate(base);
        finished
    }
}

/// A composite's digest over its finished `parts`: its kind, its seen type and its contents.
fn composed<X: Knotted>(surface: &Surface<'_, '_, X>, parts: &[ContentDigest]) -> ContentDigest {
    let (tag, contents) = match surface.parts() {
        Parts::List { .. } => (Tag::List, contents(parts.iter().copied())),
        Parts::Dict { .. } => {
            let mut hasher = DigestHasher::new(Tag::Contents);
            hasher.count(parts.len());
            for (at, part) in parts.iter().enumerate() {
                hasher
                    .digest(scalar(surface.key(at).value::<Nothing>()))
                    .digest(*part);
            }
            (Tag::Dict, hasher.finished())
        }
        Parts::Record { .. } => {
            let mut hasher = DigestHasher::new(Tag::Contents);
            hasher.count(parts.len());
            for (at, part) in parts.iter().enumerate() {
                hasher.feed(surface.name(at)).digest(*part);
            }
            (Tag::Record, hasher.finished())
        }
        Parts::Tagged { .. } => (Tag::Tagged, parts[0]),
    };
    composite(tag, surface.ktype(), contents)
}

/// A knot member's digest: its knot's, beside its index there.
fn beside_index<X: Knotted>(knot: ContentDigest, member: X) -> ContentDigest {
    member_digest(knot, member.index().index() as usize)
}

/// The digest of the member at `index` of the knot digesting as `knot`.
pub fn member_digest(knot: ContentDigest, index: usize) -> ContentDigest {
    DigestHasher::new(Tag::Member)
        .digest(knot)
        .count(index)
        .finished()
}

/// A knot's digest over the `contents` of its `count` nodes, in index order: what every knot but a
/// builtin's digests as.
pub fn knot_digest(count: usize, contents: impl Iterator<Item = ContentDigest>) -> ContentDigest {
    let mut hasher = DigestHasher::new(Tag::Knot);
    hasher.count(count);
    for content in contents {
        hasher.digest(content);
    }
    hasher.finished()
}

/// The digest of a value that is the one node of a knot of its own, whose content is `content`: a
/// module, a lone closure.
pub fn lone_member(content: ContentDigest) -> ContentDigest {
    member_digest(knot_digest(1, std::iter::once(content)), 0)
}

/// A closure's content: its body's `code` digest, the solution it is an instance at, and the
/// digest of each capture its code digest does not name, beside its slot.
pub fn function_content(
    code: ContentDigest,
    instance: &[KType],
    captures: impl Iterator<Item = (usize, ContentDigest)>,
) -> ContentDigest {
    let mut hasher = DigestHasher::new(Tag::Function);
    hasher.digest(code).count(instance.len());
    for solved in instance {
        hasher.feed(solved);
    }
    for (index, capture) in captures {
        hasher.count(index).digest(capture);
    }
    hasher.finished()
}

/// A body-born module's content: its body's `code` digest, then each capture the code digest does
/// not name, beside its slot.
pub fn body_content(
    code: ContentDigest,
    captures: impl Iterator<Item = (usize, ContentDigest)>,
) -> ContentDigest {
    let mut hasher = DigestHasher::new(Tag::Module);
    hasher.digest(code);
    for (index, capture) in captures {
        hasher.count(index).digest(capture);
    }
    hasher.finished()
}

/// Which door a view came through, a part of its content.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Operator {
    /// `:|`.
    Opaque,
    /// The re-view a nested module takes at an opaque boundary.
    Reviewed,
}

/// A view's content: its `operator` and the `application` it was built at, over its source's bare
/// member's digest and `from`, the substitution its fit solved — a signature declaring each
/// binding a manifest member. The load keys a view whose fit it cannot compute on no `from`, a key
/// no run digests.
pub fn view_content(
    operator: Operator,
    application: KType,
    source: ContentDigest,
    from: Option<KType>,
) -> ContentDigest {
    let operator = match operator {
        Operator::Opaque => Tag::Opaque,
        Operator::Reviewed => Tag::Reviewed,
    };
    DigestHasher::new(Tag::View)
        .tag(operator)
        .feed(application)
        .digest(source)
        .feed(from)
        .finished()
}

/// A knot member's digest where it is seen at `carried`, a type other than its own: the type, over
/// the bare `member`'s digest. A member's content is its knot's, never the members a type shows,
/// so a type hiding a brand keeps the brand in the digest.
pub fn retyped_content(carried: DeclaredType<KType>, member: ContentDigest) -> ContentDigest {
    DigestHasher::new(Tag::Retyped)
        .feed(carried)
        .digest(member)
        .finished()
}

/// A scalar literal's digest, as the value it evaluates to digests.
pub fn literal_digest(literal: &KLiteral<'_>) -> ContentDigest {
    match *literal {
        KLiteral::Number(number) => number_digest(number),
        KLiteral::Boolean(flag) => bool_digest(flag),
        KLiteral::Null => DigestHasher::new(Tag::Null).finished(),
        KLiteral::String(text) => str_digest(text),
    }
}

/// Code's digest as lowered syntax, equal exactly where [code equality](super::equality) calls two
/// syntaxes equal: an expression its part count, then each part's own tag and payload — a symbol
/// by its bits, a marked part by its mark, a nested expression and a container by their parts in
/// written order, a literal as [`literal_digest`] hashes it but for a zero, whose sign equality
/// ignores. Spans, sources and the structural cache are left out. Parsed syntax nests no deeper
/// than [`MAX_SYNTAX_DEPTH`](crate::parse::MAX_SYNTAX_DEPTH), so the recursion is bounded.
pub fn syntax_digest(code: &KExpression<'_>) -> ContentDigest {
    let mut hasher = DigestHasher::new(Tag::Syntax);
    syntax_expression(&mut hasher, code);
    hasher.finished()
}

/// The digest that finds the composition of `code` binding `parameters`, a callable's in its
/// shape's channel order: the code's syntax digest where it binds none — so a callable with no
/// parameters shares the composition an `EVAL` or a `BUILD` of the code finds — and otherwise a
/// digest of that and the names.
pub fn composition_digest(code: &KExpression<'_>, parameters: &[BinderSymbol]) -> ContentDigest {
    let syntax = syntax_digest(code);
    if parameters.is_empty() {
        return syntax;
    }
    let mut hasher = DigestHasher::new(Tag::Composition);
    hasher.digest(syntax).count(parameters.len());
    for name in parameters {
        hasher.feed(name);
    }
    hasher.finished()
}

fn syntax_expression(hasher: &mut DigestHasher, expression: &KExpression<'_>) {
    hasher.count(expression.parts.len());
    for part in expression.parts {
        syntax_part(hasher, &part.value);
    }
}

fn syntax_part(hasher: &mut DigestHasher, part: &ExpressionPart<'_>) {
    use ExpressionPart as Part;
    match part {
        Part::Keyword(symbol) => hasher.tag(Tag::Keyword).feed(symbol),
        Part::Identifier(symbol) => hasher.tag(Tag::Open).feed(symbol),
        Part::Type(symbol) => hasher.tag(Tag::Type).feed(symbol.symbol()),
        Part::MarkedName(mark, name) => hasher.tag(Tag::Mark).feed(mark).feed(name),
        Part::MarkedUse(mark, use_) => {
            hasher.tag(Tag::Use).feed(mark);
            syntax_expression(hasher, use_);
            hasher
        }
        Part::FilledUse(scope, keys, use_) => {
            hasher.tag(Tag::FilledUse).feed(scope).count(keys.len());
            for key in keys.iter() {
                hasher.feed(key);
            }
            syntax_expression(hasher, use_);
            hasher
        }
        Part::Splice(name) => hasher.tag(Tag::Splice).feed(name),
        Part::Expression(node)
        | Part::SigiledTypeExpr(node)
        | Part::RecordType(node)
        | Part::QuotedExpression(node) => {
            // Which grouping wraps the expression, as the variant's own index.
            let grouping = match part {
                Part::Expression(_) => 0u8,
                Part::SigiledTypeExpr(_) => 1,
                Part::RecordType(_) => 2,
                _ => 3,
            };
            hasher.tag(Tag::Group).feed(grouping);
            syntax_expression(hasher, node);
            hasher
        }
        Part::ListLiteral(items) => {
            hasher.tag(Tag::List).count(items.len());
            // A spread digests apart from the item it spreads.
            for item in items.iter() {
                hasher.feed(matches!(item, ListItem::Spread(_)));
                syntax_part(hasher, item.part());
            }
            hasher
        }
        Part::DictLiteral(pairs) => {
            hasher.tag(Tag::Dict).count(pairs.len());
            for (key, value) in pairs.iter() {
                syntax_part(hasher, key);
                syntax_part(hasher, value);
            }
            hasher
        }
        Part::RecordLiteral(fields) => {
            hasher.tag(Tag::Record).count(fields.len());
            for (name, value) in fields.iter() {
                hasher.feed(name);
                syntax_part(hasher, value);
            }
            hasher
        }
        // `-0` and `0` are one literal to equality, so both hash as `0`; a literal is never NaN.
        Part::Literal(KLiteral::Number(number)) => hasher.digest(number_digest(number + 0.0)),
        Part::Literal(literal) => hasher.digest(literal_digest(literal)),
    };
}

/// A type value's digest, from the handle of the type it names and the kind it carries: its own
/// kind's, or one a retype stamped.
pub fn type_digest(named: KType, kind: KType) -> ContentDigest {
    DigestHasher::new(Tag::Type)
        .feed(named)
        .feed(kind)
        .finished()
}

fn number_digest(number: f64) -> ContentDigest {
    DigestHasher::new(Tag::Number)
        .feed(number.to_bits())
        .finished()
}

fn bool_digest(flag: bool) -> ContentDigest {
    DigestHasher::new(Tag::Bool).feed(flag).finished()
}

fn str_digest(text: &str) -> ContentDigest {
    DigestHasher::new(Tag::Str).text(text.as_bytes()).finished()
}

/// A string view's digest: its flat string's, its bytes fed in reading order.
fn view_digest(text: Text<'_>) -> ContentDigest {
    let mut hasher = DigestHasher::new(Tag::Str);
    hasher.count(text.byte_len());
    for point in text.chars() {
        hasher.0.update(point.encode_utf8(&mut [0; 4]).as_bytes());
    }
    hasher.finished()
}

/// An edge's digest inside its knot's: its index, since it names a sibling the knot's digest
/// covers.
pub fn edge(edge: Edge) -> ContentDigest {
    edge_at(edge.index() as usize)
}

/// The digest of an edge to the node at `index` of its knot.
pub fn edge_at(index: usize) -> ContentDigest {
    DigestHasher::new(Tag::Edge).count(index).finished()
}

/// A leaf's digest, from its payload. Only a scalar, a string or a type value is one.
pub(super) fn scalar<X>(value: Value<'_, X>) -> ContentDigest {
    match value {
        Value::Number(number) => number_digest(number),
        Value::Bool(flag) => bool_digest(flag),
        Value::Null => literal_digest(&KLiteral::Null),
        Value::Str(text) => str_digest(text),
        Value::StrView(view) => view_digest(view.text()),
        Value::Type(value) => type_digest(value.handle(), value.ktype()),
        _ => unreachable!("a composite or a knot member is no leaf"),
    }
}

/// The kind's tag and the address of the resident a plain composite `value` opens as.
fn resident<X: Knotted>(value: Value<'_, X>) -> (Tag, usize) {
    fn at<T>(resident: &T) -> usize {
        std::ptr::from_ref(resident).addr()
    }
    match value {
        Value::List(list) => (Tag::List, at(list)),
        Value::Dict(dict) => (Tag::Dict, at(dict)),
        Value::Record(record) => (Tag::Record, at(record)),
        Value::Tagged(tagged) => (Tag::Tagged, at(tagged)),
        _ => unreachable!("only a plain composite opens a resident"),
    }
}

#[cfg(test)]
thread_local! {
    /// How many parts the content-digest walk has hashed, memo hits excluded.
    pub static DIGEST_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many digest demands have been opened: one per [`Digests`] memo.
    pub static DIGEST_DEMANDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
