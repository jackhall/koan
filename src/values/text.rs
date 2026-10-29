//! A string's text, read in code points: a flat string's bytes, or a [`StrView`] — a slice of
//! one, forward or backward, that lays none of its bytes. See
//! [README.md § Views](README.md#views).
//!
//! Nothing concatenates strings, so a string view is one stretch of one flat string, its
//! **anchor**: a slice of a view shares its anchor. A view made from a flat string counts the
//! anchor's code points once and lays a **crumb** every [`CRUMB`] code points — the byte offset
//! where that code point starts — unless the anchor is ASCII, whose code point is its byte, so
//! finding a code point decodes at most `CRUMB - 1` past a crumb. UTF-8 reads backward as cheaply
//! as forward, so a backward view is as cheap as a forward one.
//!
//! Every reader takes a string value's [`Text`] ([`Value::as_text`]): its length, its code points
//! in reading order, and the bytes a flat copy of it writes. A copy writes a view flat
//! ([crossing.rs](super::crossing)).

use std::fmt;

use crate::memory::{BumpAllocator, BumpVec, Lays, Writer, resident};

use super::Value;

/// How many code points of an anchor one crumb spans.
const CRUMB: usize = 64;

/// `length` code points of an anchor string from code point `from`, read forward or backward.
/// Never empty: an empty slice is the flat empty string.
#[derive(Debug)]
pub struct StrView<'cell> {
    anchor: &'cell str,
    /// The byte offset of the anchor's code point `CRUMB·k`, for every such code point up to its
    /// length; `None` for an ASCII anchor.
    crumbs: Option<&'cell [usize]>,
    from: usize,
    length: usize,
    /// The byte offset of code point `from`, and how many bytes the view shows.
    start: usize,
    bytes: usize,
    backward: bool,
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

/// A string value's text: flat bytes, or a view over another string's.
#[derive(Clone, Copy, Debug)]
pub enum Text<'cell> {
    Flat(&'cell str),
    View(&'cell StrView<'cell>),
}

impl<'cell> Text<'cell> {
    /// The bytes it shows, in the anchor's order: all of it where it reads forward.
    fn shown(self) -> &'cell str {
        match self {
            Text::Flat(text) => text,
            Text::View(view) => &view.anchor[view.start..view.start + view.bytes],
        }
    }

    /// Whether it reads its shown bytes backward.
    fn backward(self) -> bool {
        matches!(self, Text::View(view) if view.backward)
    }

    /// Its text as one run of bytes, where it reads forward: a flat string, or a forward view's
    /// stretch of its anchor.
    pub fn forward(self) -> Option<&'cell str> {
        (!self.backward()).then(|| self.shown())
    }

    /// Its text as one run of bytes: a forward run as it is, a backward view's code points laid in
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
            Text::View(view) => view.length,
        }
    }

    pub fn is_empty(self) -> bool {
        self.shown().is_empty()
    }

    /// The bytes a flat copy of it writes.
    pub fn byte_len(self) -> usize {
        self.shown().len()
    }

    /// Its code points, in reading order.
    pub fn chars(self) -> Chars<'cell> {
        Chars {
            chars: self.shown().chars(),
            backward: self.backward(),
        }
    }

    /// The code point at `at`, written flat through `writer`, or `None` past its end: O(1) in a
    /// view, one scan up to it in a flat string.
    pub fn code_point<X>(self, writer: Writer<'cell, '_>, at: usize) -> Option<Value<'cell, X>> {
        let point = match self {
            Text::Flat(text) => {
                scanned();
                text.chars().nth(at)?
            }
            Text::View(view) => {
                if at >= view.length {
                    return None;
                }
                let at = match view.backward {
                    false => view.from + at,
                    true => view.from + view.length - 1 - at,
                };
                let start = byte_at(view.anchor, view.crumbs, at);
                view.anchor[start..]
                    .chars()
                    .next()
                    .expect("a code point in range")
            }
        };
        Some(Value::Str(writer.text(point.encode_utf8(&mut [0; 4]))))
    }

    /// It as a slice's source: a view's own anchor, or a flat string counted and its crumbs laid
    /// through `writer` in one scan.
    pub(crate) fn anchored(
        self,
        writer: Writer<'cell, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Anchored<'cell> {
        match self {
            Text::Flat(text) => {
                let (len, crumbs) = crumbs(writer, scratch, text);
                Anchored {
                    anchor: text,
                    crumbs,
                    from: 0,
                    len,
                    backward: false,
                }
            }
            Text::View(view) => Anchored {
                anchor: view.anchor,
                crumbs: view.crumbs,
                from: view.from,
                len: view.length,
                backward: view.backward,
            },
        }
    }
}

/// A text as a slice's source: `len` code points of `anchor` from code point `from`, read
/// backward where `backward` is set, with the anchor's crumbs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Anchored<'cell> {
    anchor: &'cell str,
    crumbs: Option<&'cell [usize]>,
    from: usize,
    len: usize,
    backward: bool,
}

impl<'cell> Anchored<'cell> {
    /// How many code points the text holds.
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Code points `[from, to)` of the text, read backward where `backward` is set: a view over
    /// its anchor, or the flat empty string where it holds none.
    pub(crate) fn sliced<X>(
        &self,
        writer: Writer<'cell, '_>,
        (from, to): (usize, usize),
        backward: bool,
    ) -> Value<'cell, X> {
        if from == to {
            return Value::Str(writer.text(""));
        }
        let at = match self.backward {
            false => self.from + from,
            true => self.from + self.len - to,
        };
        let length = to - from;
        let start = byte_at(self.anchor, self.crumbs, at);
        let end = byte_at(self.anchor, self.crumbs, at + length);
        Value::StrView(resident(
            writer,
            StrView {
                anchor: self.anchor,
                crumbs: self.crumbs,
                from: at,
                length,
                start,
                bytes: end - start,
                backward: self.backward ^ backward,
            },
        ))
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

/// A text's code points in reading order.
pub struct Chars<'cell> {
    chars: std::str::Chars<'cell>,
    backward: bool,
}

impl Iterator for Chars<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        match self.backward {
            false => self.chars.next(),
            true => self.chars.next_back(),
        }
    }
}

impl<'cell> StrView<'cell> {
    /// Its text.
    pub fn text(&'cell self) -> Text<'cell> {
        Text::View(self)
    }
}
