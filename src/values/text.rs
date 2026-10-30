//! A string's text, read in code points: a flat string's bytes, or a [`StrView`] — a tree of
//! stretches of flat strings, each read forward or backward, that lays none of their bytes. See
//! [README.md § Views](README.md#views).
//!
//! A string view is a [view tree](super::view) whose leaf is a [`TextStretch`]: some code points of
//! one flat string, its **anchor**, beside the anchor's crumbs. A slice splits the tree and a
//! [`Concatenation`] joins trees, so every edit shares its sources' anchors. A stretch made from a
//! flat string counts the anchor's code points once and lays a **crumb** every [`CRUMB`] code
//! points — the byte offset where that code point starts — unless the anchor is ASCII, whose code
//! point is its byte, so finding a code point decodes at most `CRUMB - 1` past a crumb. UTF-8 reads
//! backward as cheaply as forward, so a backward stretch is as cheap as a forward one.
//!
//! Every reader takes a string value's [`Text`] ([`Value::as_text`]): its length, its code points
//! in reading order, and the bytes a flat copy of it writes. A copy writes a view flat
//! ([crossing.rs](super::crossing)).

use std::fmt;

use crate::memory::{BumpAllocator, BumpVec, Lays, Writer, resident};

use super::Value;
use super::view::{self, Leaf, Leaves, Summary};

/// How many code points of an anchor one crumb spans.
const CRUMB: usize = 64;

/// `length` code points of an anchor string from code point `from`, read forward or backward: a
/// string view's leaf. Never empty.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TextStretch<'cell> {
    anchor: &'cell str,
    /// The byte offset of the anchor's code point `CRUMB·k`, for every such code point up to its
    /// length; `None` for an ASCII anchor.
    crumbs: Option<&'cell [usize]>,
    from: usize,
    length: usize,
    /// The byte offset of code point `from`, and how many bytes the stretch shows.
    start: usize,
    bytes: usize,
    backward: bool,
}

/// What a string view's subtree memoizes: its code points, and the bytes a flat copy writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextSummary {
    length: usize,
    bytes: usize,
}

/// A string view's tree, named without projections so a view stays covariant in its lifetime.
pub(crate) type TextTree<'cell> = view::Node<'cell, TextStretch<'cell>, TextSummary, ()>;

/// A string view: a tree of stretches, never empty.
#[derive(Debug)]
pub struct StrView<'cell> {
    tree: &'cell TextTree<'cell>,
}

#[cfg(test)]
thread_local! {
    /// How many code points finding a code point's byte offset has decoded past a crumb.
    pub(crate) static DECODED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many scans over a flat string's bytes reading it has made.
    pub(crate) static SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Count one scan over a flat string's bytes, in a test build.
fn scanned() {
    #[cfg(test)]
    SCANS.with(|scans| scans.set(scans.get() + 1));
}

/// The byte offset in `anchor` where code point `at` starts, read off `crumbs`; the anchor's
/// length where `at` is its code-point count.
fn byte_at(anchor: &str, crumbs: Option<&[usize]>, at: usize) -> usize {
    let Some(crumbs) = crumbs else {
        return at;
    };
    let base = crumbs[at / CRUMB];
    #[cfg(test)]
    DECODED.with(|decoded| decoded.set(decoded.get() + at % CRUMB));
    anchor[base..]
        .char_indices()
        .nth(at % CRUMB)
        .map_or(anchor.len(), |(offset, _)| base + offset)
}

impl<'cell> TextStretch<'cell> {
    /// Anchor code points `[from, to)`, whose byte offsets are `start` and `end`.
    fn new(
        anchor: &'cell str,
        crumbs: Option<&'cell [usize]>,
        (from, to): (usize, usize),
        (start, end): (usize, usize),
        backward: bool,
    ) -> Self {
        TextStretch {
            anchor,
            crumbs,
            from,
            length: to - from,
            start,
            bytes: end - start,
            backward,
        }
    }

    /// The bytes it shows, in the anchor's order.
    fn shown(&self) -> &'cell str {
        &self.anchor[self.start..self.start + self.bytes]
    }

    /// Its code point `at`, in reading order.
    fn code_point(&self, at: usize) -> char {
        let at = match self.backward {
            false => self.from + at,
            true => self.from + self.length - 1 - at,
        };
        let start = byte_at(self.anchor, self.crumbs, at);
        self.anchor[start..]
            .chars()
            .next()
            .expect("a code point in range")
    }
}

impl Summary<()> for TextSummary {
    fn len(&self) -> usize {
        self.length
    }

    fn joined(self, right: Self) -> Self {
        TextSummary {
            length: self.length + right.length,
            bytes: self.bytes + right.bytes,
        }
    }

    fn reversed(self) -> Self {
        self
    }

    fn waits(&self) -> bool {
        false
    }

    fn under(self, (): ()) -> Self {
        self
    }
}

impl Leaf for TextStretch<'_> {
    type Summary = TextSummary;
    type Lazy = ();

    fn summary(&self) -> TextSummary {
        TextSummary {
            length: self.length,
            bytes: self.bytes,
        }
    }

    fn split(self, at: usize) -> (Option<Self>, Option<Self>) {
        let (from, to) = (self.from, self.from + self.length);
        // Reading backward, the first `at` code points are the anchor's last `at`.
        let cut = match self.backward {
            false => from + at,
            true => to - at,
        };
        let (start, end) = (self.start, self.start + self.bytes);
        let middle = byte_at(self.anchor, self.crumbs, cut);
        let stretch = |points: (usize, usize), bytes| {
            (points.0 < points.1)
                .then(|| TextStretch::new(self.anchor, self.crumbs, points, bytes, self.backward))
        };
        let (low, high) = (
            stretch((from, cut), (start, middle)),
            stretch((cut, to), (middle, end)),
        );
        match self.backward {
            false => (low, high),
            true => (high, low),
        }
    }

    fn reversed(self) -> Self {
        TextStretch {
            backward: !self.backward,
            ..self
        }
    }

    fn under(self, (): ()) -> Self {
        self
    }

    #[cfg(test)]
    fn holds_its_laws(&self) -> bool {
        let start = byte_at(self.anchor, self.crumbs, self.from);
        let end = byte_at(self.anchor, self.crumbs, self.from + self.length);
        self.length > 0 && self.start == start && self.bytes == end - start
    }
}

/// A string value's text: flat bytes, or a view over others'.
#[derive(Clone, Copy, Debug)]
pub enum Text<'cell> {
    Flat(&'cell str),
    View(&'cell StrView<'cell>),
}

impl<'cell> Text<'cell> {
    /// Its text as one run of bytes, where it reads forward: a flat string, or a one-stretch
    /// forward view's stretch of its anchor.
    pub fn forward(self) -> Option<&'cell str> {
        match self {
            Text::Flat(text) => Some(text),
            Text::View(view) => match *view.tree {
                view::Node::Leaf(stretch) if !stretch.backward => Some(stretch.shown()),
                _ => None,
            },
        }
    }

    /// Its text as one run of bytes: a forward run as it is, and otherwise its code points laid in
    /// reading order through `lay`.
    pub fn laid(self, lay: impl Lays<'cell>) -> &'cell str {
        if let Some(text) = self.forward() {
            return text;
        }
        let mut bytes = self.chars().flat_map(|point| {
            let mut buffer = [0; 4];
            let len = point.encode_utf8(&mut buffer).len();
            buffer.into_iter().take(len)
        });
        let laid = lay.fill(self.byte_len(), |_| {
            bytes.next().expect("as many bytes as shown")
        });
        std::str::from_utf8(laid).expect("code points in any order are UTF-8")
    }

    /// How many code points it holds: a view's memo, or a count over a flat string's bytes.
    pub fn len(self) -> usize {
        match self {
            Text::Flat(text) => {
                scanned();
                text.chars().count()
            }
            Text::View(view) => view.tree.len(),
        }
    }

    pub fn is_empty(self) -> bool {
        match self {
            Text::Flat(text) => text.is_empty(),
            Text::View(_) => false,
        }
    }

    /// The bytes a flat copy of it writes.
    pub fn byte_len(self) -> usize {
        match self {
            Text::Flat(text) => text.len(),
            Text::View(view) => view.tree.summary().bytes,
        }
    }

    /// Its code points, in reading order.
    pub fn chars(self) -> Chars<'cell> {
        match self {
            Text::Flat(text) => Chars {
                chars: text.chars(),
                backward: false,
                rest: None,
            },
            Text::View(view) => Chars {
                chars: "".chars(),
                backward: false,
                rest: Some(view.tree.leaves()),
            },
        }
    }

    /// The code point at `at`, written flat through `writer`, or `None` past its end: O(log n) in
    /// a view of n stretches, one scan up to it in a flat string.
    pub fn code_point<X>(self, writer: Writer<'cell, '_>, at: usize) -> Option<Value<'cell, X>> {
        let point = match self {
            Text::Flat(text) => {
                scanned();
                text.chars().nth(at)?
            }
            Text::View(view) => {
                if at >= view.tree.len() {
                    return None;
                }
                let (stretch, at, flip, _) = view.tree.locate(at);
                let stretch = if flip { stretch.reversed() } else { stretch };
                stretch.code_point(at)
            }
        };
        Some(Value::Str(writer.text(point.encode_utf8(&mut [0; 4]))))
    }

    /// It as a slice's source: a view's own tree, or a flat string counted and its crumbs laid
    /// through `writer` in one scan.
    pub(crate) fn anchored(
        self,
        writer: Writer<'cell, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Anchored<'cell> {
        match self {
            Text::Flat(text) => {
                let (len, crumbs) = crumbs(writer, scratch, text);
                Anchored::Flat {
                    anchor: text,
                    crumbs,
                    len,
                }
            }
            Text::View(view) => Anchored::View(view.tree),
        }
    }
}

/// A text as a slice's source: a flat string beside its code-point count and crumbs, or a view's
/// tree.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Anchored<'cell> {
    Flat {
        anchor: &'cell str,
        crumbs: Option<&'cell [usize]>,
        len: usize,
    },
    View(&'cell TextTree<'cell>),
}

impl<'cell> Anchored<'cell> {
    /// How many code points the text holds.
    pub(crate) fn len(&self) -> usize {
        match self {
            Anchored::Flat { len, .. } => *len,
            Anchored::View(tree) => tree.len(),
        }
    }

    /// Code points `[from, to)` of the text, read backward where `backward` is set: a view over
    /// its anchors, or the flat empty string where it holds none.
    pub(crate) fn sliced<X>(
        &self,
        writer: Writer<'cell, '_>,
        (from, to): (usize, usize),
        backward: bool,
    ) -> Value<'cell, X> {
        if from == to {
            return Value::Str(writer.text(""));
        }
        let tree = match *self {
            Anchored::Flat { anchor, crumbs, .. } => {
                let bytes = (byte_at(anchor, crumbs, from), byte_at(anchor, crumbs, to));
                let stretch = TextStretch::new(anchor, crumbs, (from, to), bytes, backward);
                view::of(writer, stretch)
            }
            Anchored::View(tree) => {
                let tree = view::sliced(writer, tree, from, to).expect("a non-empty slice");
                match backward {
                    false => tree,
                    true => view::reversed(writer, tree),
                }
            }
        };
        Value::StrView(resident(writer, StrView { tree }))
    }
}

/// How many code points `text` holds, and its crumbs, counted in one scan: the crumbs gathered in
/// `scratch` and laid through `writer` only where the text is not ASCII, whose count is its bytes.
fn crumbs<'cell>(
    writer: Writer<'cell, '_>,
    scratch: BumpAllocator<'_>,
    text: &str,
) -> (usize, Option<&'cell [usize]>) {
    scanned();
    let mut gathered = BumpVec::with_capacity_in(text.len() / CRUMB + 1, scratch);
    let mut count = 0;
    for (offset, _) in text.char_indices() {
        if count % CRUMB == 0 {
            gathered.push(offset);
        }
        count += 1;
    }
    if count == text.len() {
        return (count, None);
    }
    if count % CRUMB == 0 {
        gathered.push(text.len());
    }
    (count, Some(writer.fill(gathered.len(), |at| gathered[at])))
}

/// A string spliced from texts in order: the flat empty string where all of them are empty, and
/// otherwise a view sharing their stretches. Each flat text is counted and its crumbs laid in one
/// scan; each view joins in O(log n) nodes.
pub struct Concatenation<'cell> {
    tree: Option<&'cell TextTree<'cell>>,
}

impl<'cell> Concatenation<'cell> {
    pub fn new() -> Self {
        Concatenation { tree: None }
    }

    /// Append `text`.
    pub fn push(
        &mut self,
        writer: Writer<'cell, '_>,
        scratch: BumpAllocator<'_>,
        text: Text<'cell>,
    ) {
        let tree = match text {
            Text::Flat("") => return,
            Text::Flat(text) => {
                let (len, crumbs) = crumbs(writer, scratch, text);
                let stretch = TextStretch::new(text, crumbs, (0, len), (0, text.len()), false);
                view::of(writer, stretch)
            }
            Text::View(view) => view.tree,
        };
        self.tree = view::joined(writer, self.tree, Some(tree));
    }

    /// The string spliced.
    pub fn finish<X>(self, writer: Writer<'cell, '_>) -> Value<'cell, X> {
        match self.tree {
            None => Value::Str(writer.text("")),
            Some(tree) => Value::StrView(resident(writer, StrView { tree })),
        }
    }
}

impl Default for Concatenation<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for Text<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self.forward(), other.forward()) {
            (Some(left), Some(right)) => left == right,
            _ => self.byte_len() == other.byte_len() && self.chars().eq(other.chars()),
        }
    }
}

impl fmt::Display for Text<'_> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.forward() {
            Some(text) => out.write_str(text),
            None => self
                .chars()
                .try_for_each(|point| fmt::Write::write_char(out, point)),
        }
    }
}

/// A text's code points in reading order: those of the stretch at hand, then of the view's stretches
/// still to read.
pub struct Chars<'cell> {
    chars: std::str::Chars<'cell>,
    backward: bool,
    rest: Option<Leaves<'cell, TextStretch<'cell>, TextSummary, ()>>,
}

impl Iterator for Chars<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        loop {
            let point = match self.backward {
                false => self.chars.next(),
                true => self.chars.next_back(),
            };
            if point.is_some() {
                return point;
            }
            let (stretch, flip, _) = self.rest.as_mut()?.next()?;
            self.chars = stretch.shown().chars();
            self.backward = stretch.backward ^ flip;
        }
    }
}

impl<'cell> StrView<'cell> {
    /// Its text.
    pub fn text(&'cell self) -> Text<'cell> {
        Text::View(self)
    }

    /// Whether its tree holds its laws.
    #[cfg(test)]
    pub(crate) fn holds_its_laws(&self) -> bool {
        self.tree.holds_its_laws()
    }
}
