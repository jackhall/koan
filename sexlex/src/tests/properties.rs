//! Random trees, rendered under random layouts, read back. The renderer is the executable
//! statement of the layout rules; the generator produces only trees the rules can express (see
//! [`normalize`] for the adjacency facts it forces to hold).

use proptest::prelude::*;

use crate::{Item, Kind, Node, Span, read};

// --- Generators ---

/// An atom: anything that is not whitespace, a bracket, a quote or a comma. `~` is excluded only
/// to keep failure renders readable.
fn atom() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_.:#$=<>?!+*/|-]{1,6}"
}

/// A single-quoted body: plain characters, escaped quotes and escaped backslashes.
fn str_body() -> impl Strategy<Value = String> {
    r"([a-z ]|\\'|\\\\){0,4}"
}

fn no_span() -> Span {
    Span { start: 0, end: 0 }
}

fn item(node: Node<'static>, glued: bool) -> Item<'static> {
    Item {
        node,
        span: no_span(),
        glued,
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// An item that can appear in a line's inline run. `flat` says whether an enclosing `[` / `{`
/// forbids paren bodies with layout children.
fn inline_item(depth: u32, flat: bool) -> BoxedStrategy<Item<'static>> {
    let leaf = prop_oneof![
        5 => (atom(), any::<bool>()).prop_map(|(a, g)| item(Node::Atom(leak(a)), g)),
        2 => (str_body(), any::<bool>()).prop_map(|(b, g)| item(
            Node::Str {
                quote: '\'',
                body: leak(b)
            },
            g
        )),
        1 => any::<bool>().prop_map(|g| item(Node::Comma, g)),
    ];
    if depth == 0 {
        return leaf.boxed();
    }
    let bracketed = prop_oneof![Just(Kind::Bracket), Just(Kind::Brace)]
        .prop_flat_map(move |kind| {
            (
                Just(kind),
                prop::collection::vec(inline_item(depth - 1, true), 0..4),
                any::<bool>(),
            )
        })
        .prop_map(|(kind, items, g)| item(Node::Group { kind, items }, g));
    let paren = (
        prop::collection::vec(inline_item(depth - 1, flat), 0..4),
        if flat {
            Just(Vec::new()).boxed()
        } else {
            prop::collection::vec(layout_item(depth - 1), 0..3).boxed()
        },
        any::<bool>(),
    )
        .prop_map(|(mut inline, children, g)| {
            // A trailing comma before a body line would join the line instead of nesting it.
            if !children.is_empty()
                && matches!(
                    inline.last(),
                    Some(Item {
                        node: Node::Comma,
                        ..
                    })
                )
            {
                inline.push(item(Node::Atom("x"), false));
            }
            inline.extend(children);
            item(
                Node::Group {
                    kind: Kind::Paren,
                    items: inline,
                },
                g,
            )
        });
    prop_oneof![6 => leaf, 2 => bracketed, 2 => paren].boxed()
}

/// A layout line: a non-empty inline run whose last item is not a comma (a trailing comma would
/// join the next line), then its child lines.
fn layout_item(depth: u32) -> BoxedStrategy<Item<'static>> {
    let children = if depth == 0 {
        Just(Vec::new()).boxed()
    } else {
        prop::collection::vec(layout_item(depth - 1), 0..3).boxed()
    };
    (
        prop::collection::vec(inline_item(depth, false), 1..4),
        children,
    )
        .prop_map(|(mut inline, children)| {
            if matches!(
                inline.last(),
                Some(Item {
                    node: Node::Comma,
                    ..
                })
            ) {
                inline.push(item(Node::Atom("x"), false));
            }
            inline.extend(children);
            item(
                Node::Group {
                    kind: Kind::Layout,
                    items: inline,
                },
                false,
            )
        })
        .boxed()
}

fn trees() -> impl Strategy<Value = Vec<Item<'static>>> {
    prop::collection::vec(layout_item(2), 0..4)
}

/// Force the adjacency facts the source text cannot express: nothing is glued to a layout
/// group or to nothing, a layout group is glued to nothing, and two atoms cannot touch without
/// merging. Spans are zeroed so a parsed tree compares by structure.
fn normalize(items: &mut [Item<'static>]) {
    let n = items.len();
    for i in 0..n {
        let next_is_layout = items.get(i + 1).is_some_and(|next| {
            matches!(
                next.node,
                Node::Group {
                    kind: Kind::Layout,
                    ..
                }
            )
        });
        let atom_atom = matches!(items[i].node, Node::Atom(_))
            && items
                .get(i + 1)
                .is_some_and(|next| matches!(next.node, Node::Atom(_)));
        let is_layout = matches!(
            items[i].node,
            Node::Group {
                kind: Kind::Layout,
                ..
            }
        );
        if i + 1 == n || next_is_layout || atom_atom || is_layout {
            items[i].glued = false;
        }
        items[i].span = no_span();
        if let Node::Group { items: inner, .. } = &mut items[i].node {
            normalize(inner);
        }
    }
}

fn strip_spans(items: &mut [Item<'_>]) {
    for item in items {
        item.span = no_span();
        if let Node::Group { items, .. } = &mut item.node {
            strip_spans(items);
        }
    }
}

// --- Renderer ---

/// A tape of layout choices. Each `pick` consumes one byte; an exhausted tape answers `0`, which
/// is always the canonical choice, so shrinking a tape shrinks toward the canonical layout.
struct Tape {
    bytes: Vec<u8>,
    at: usize,
}

impl Tape {
    fn pick(&mut self, n: usize) -> usize {
        let b = self.bytes.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        b as usize % n
    }
}

fn spaces(out: &mut String, n: usize) {
    out.extend(std::iter::repeat_n(' ', n));
}

/// Blank lines, comment-only lines and trailing whitespace between lines are layout noise.
fn line_noise(out: &mut String, tape: &mut Tape) {
    match tape.pick(6) {
        1 => out.push_str("   \n"),
        2 => out.push('\n'),
        3 => out.push_str("\t\n"),
        4 => out.push_str("   ; ( ] ' \"\n"),
        5 => out.push_str("\t; note\n"),
        _ => {}
    }
}

/// A comment at a line's end. It holds unmatched brackets and quotes, which a comment makes
/// inert. Generated atoms and string bodies hold no `;`, so in rendered source every `;` starts a
/// comment.
const COMMENT: &str = " ; a ( ] } ' \" note";

/// End a line, maybe after a comment.
fn line_end(out: &mut String, tape: &mut Tape) {
    match tape.pick(3) {
        1 => out.push_str(COMMENT),
        // Glued straight onto the item before it.
        2 => out.push_str(";)'"),
        _ => {}
    }
    out.push('\n');
}

fn render_source(items: &[Item<'_>], tape: &mut Tape) -> String {
    let mut out = String::new();
    for line in items {
        render_layout(line, 0, tape, &mut out);
    }
    out
}

fn split_line<'a, 's>(items: &'a [Item<'s>]) -> (&'a [Item<'s>], &'a [Item<'s>]) {
    let first_child = items
        .iter()
        .position(|it| {
            matches!(
                it.node,
                Node::Group {
                    kind: Kind::Layout,
                    ..
                }
            )
        })
        .unwrap_or(items.len());
    items.split_at(first_child)
}

fn render_layout(line: &Item<'_>, indent: usize, tape: &mut Tape, out: &mut String) {
    let Node::Group { items, .. } = &line.node else {
        panic!("a layout line is a group")
    };
    let (inline, children) = split_line(items);
    line_noise(out, tape);
    spaces(out, indent);
    render_inline(inline, indent, tape, out);
    if tape.pick(3) == 1 {
        out.push_str("  ");
    }
    line_end(out, tape);
    let step = 2 + 2 * tape.pick(2);
    for child in children {
        render_layout(child, indent + step, tape, out);
    }
}

/// Render one run of items. Between two items: nothing when glued; otherwise one or more spaces,
/// or (in a flat group) a line break at any indentation, or (after a comma) a continuation break.
fn render_inline(items: &[Item<'_>], indent: usize, tape: &mut Tape, out: &mut String) {
    for (i, item) in items.iter().enumerate() {
        render_item(item, indent, tape, out);
        if i + 1 == items.len() || item.glued {
            continue;
        }
        let comma = matches!(item.node, Node::Comma);
        match tape.pick(4) {
            1 => spaces(out, 3),
            2 if comma => {
                line_end(out, tape);
                line_noise(out, tape);
                spaces(out, tape.pick(6));
            }
            _ => out.push(' '),
        }
    }
}

fn render_flat(items: &[Item<'_>], tape: &mut Tape, out: &mut String) {
    for (i, item) in items.iter().enumerate() {
        let glued_to_previous = i > 0 && items[i - 1].glued;
        if !glued_to_previous && tape.pick(3) == 1 {
            line_end(out, tape);
            line_noise(out, tape);
            spaces(out, tape.pick(5));
        }
        render_item(item, 0, tape, out);
        if i + 1 == items.len() || item.glued {
            continue;
        }
        out.push(' ');
    }
    if tape.pick(3) == 1 {
        line_end(out, tape);
        spaces(out, tape.pick(5));
    }
}

fn render_item(item: &Item<'_>, indent: usize, tape: &mut Tape, out: &mut String) {
    match &item.node {
        Node::Atom(text) => out.push_str(text),
        Node::Str { quote, body } => {
            out.push(*quote);
            out.push_str(body);
            out.push(*quote);
        }
        Node::Comma => out.push(','),
        Node::Group { kind, items } => {
            let (open, close) = kind.delimiters().expect("inline groups are bracketed");
            out.push(open);
            if *kind != Kind::Paren {
                render_flat(items, tape, out);
                out.push(close);
                return;
            }
            let (inline, children) = split_line(items);
            render_inline(inline, indent, tape, out);
            if children.is_empty() {
                if tape.pick(3) == 1 {
                    // The closer on its own line, at or beyond the opening line's indentation.
                    line_end(out, tape);
                    spaces(out, indent + 2 * tape.pick(3));
                }
                out.push(close);
                return;
            }
            line_end(out, tape);
            let step = 2 + 2 * tape.pick(2);
            for child in children {
                render_layout(child, indent + step, tape, out);
            }
            match tape.pick(3) {
                1 => {
                    // The closer at the end of the last body line, before any comment there.
                    let popped = out.pop();
                    debug_assert_eq!(popped, Some('\n'));
                    let line_start = out.rfind('\n').map_or(0, |at| at + 1);
                    if let Some(at) = out[line_start..].find(';') {
                        out.truncate(line_start + at);
                    }
                }
                2 => spaces(out, indent + 2 * tape.pick(3)),
                _ => spaces(out, indent),
            }
            out.push(close);
        }
    }
}

// --- Span checks ---

fn check_spans(items: &[Item<'_>], source: &str, parent: Option<Span>) {
    let mut previous_end = parent.map_or(0, |p| p.start);
    for item in items {
        let span = item.span;
        assert!(
            span.start >= previous_end,
            "siblings are ordered and disjoint"
        );
        if let Some(p) = parent {
            assert!(
                span.start >= p.start && span.end <= p.end,
                "child within parent"
            );
        }
        let text = span.slice(source);
        match &item.node {
            Node::Atom(atom) => assert_eq!(&text, atom),
            Node::Str { quote, body } => {
                assert_eq!(text, format!("{quote}{body}{quote}"));
            }
            Node::Comma => assert_eq!(text, ","),
            Node::Group { kind, items: inner } => {
                match kind.delimiters() {
                    Some((open, close)) => {
                        assert!(text.starts_with(open) && text.ends_with(close));
                    }
                    None => {
                        let first = inner.first().expect("a layout group is non-empty");
                        let last = inner.last().expect("a layout group is non-empty");
                        assert_eq!(span.start, first.span.start);
                        assert_eq!(span.end, last.span.end);
                    }
                }
                check_spans(inner, source, Some(span));
            }
        }
        previous_end = span.end;
    }
}

fn group_spans(items: &[Item<'_>], out: &mut Vec<(Kind, Span)>) {
    for item in items {
        if let Node::Group { kind, items: inner } = &item.node {
            if *kind != Kind::Layout {
                out.push((*kind, item.span));
            }
            group_spans(inner, out);
        }
    }
}

fn string_spans(items: &[Item<'_>], out: &mut Vec<Span>) {
    for item in items {
        match &item.node {
            Node::Str { .. } => out.push(item.span),
            Node::Group { items: inner, .. } => string_spans(inner, out),
            _ => {}
        }
    }
}

/// Shift every span in `items`, nested ones included, by `base`.
fn shifted(items: &mut [Item<'_>], base: u32) {
    for item in items {
        item.span = Span {
            start: item.span.start + base,
            end: item.span.end + base,
        };
        if let Node::Group { items, .. } = &mut item.node {
            shifted(items, base);
        }
    }
}

// --- Properties ---

proptest! {
    /// The canonical rendering reads back as the tree it was rendered from.
    #[test]
    fn canonical_layout_round_trips(mut tree in trees()) {
        normalize(&mut tree);
        let source = render_source(&tree, &mut Tape { bytes: Vec::new(), at: 0 });
        let mut parsed = read(&source).unwrap_or_else(|e| panic!("{source:?}: {e}"));
        strip_spans(&mut parsed);
        prop_assert_eq!(parsed, tree, "source:\n{}", source);
    }

    /// Any layout the rules admit reads back as the same tree: where a closer sits, how deep a
    /// body indents, where a bracket's contents break, and whether a comma breaks the line are
    /// all layout, not structure.
    #[test]
    fn layout_never_changes_structure(mut tree in trees(), bytes in prop::collection::vec(any::<u8>(), 0..96)) {
        normalize(&mut tree);
        let source = render_source(&tree, &mut Tape { bytes, at: 0 });
        let mut parsed = read(&source).unwrap_or_else(|e| panic!("{source:?}: {e}"));
        strip_spans(&mut parsed);
        prop_assert_eq!(parsed, tree, "source:\n{}", source);
    }

    /// Every span slices back to its own text, delimiters included; children lie within their
    /// parent and siblings are ordered and disjoint.
    #[test]
    fn spans_index_their_own_text(mut tree in trees(), bytes in prop::collection::vec(any::<u8>(), 0..96)) {
        normalize(&mut tree);
        let source = render_source(&tree, &mut Tape { bytes, at: 0 });
        let parsed = read(&source).unwrap_or_else(|e| panic!("{source:?}: {e}"));
        check_spans(&parsed, &source, None);
    }

    /// Deleting any closer, or inserting a closer anywhere outside a string or a comment, is
    /// rejected.
    #[test]
    fn an_unbalanced_edit_is_rejected(
        mut tree in trees(),
        bytes in prop::collection::vec(any::<u8>(), 0..96),
        which in any::<prop::sample::Index>(),
        at in any::<prop::sample::Index>(),
        closer in prop::sample::select(vec![')', ']', '}']),
    ) {
        normalize(&mut tree);
        let source = render_source(&tree, &mut Tape { bytes, at: 0 });
        let parsed = read(&source).unwrap_or_else(|e| panic!("{source:?}: {e}"));

        let mut groups = Vec::new();
        group_spans(&parsed, &mut groups);
        if !groups.is_empty() {
            let (_, span) = groups[which.index(groups.len())];
            let mut deleted = source.clone();
            deleted.remove(span.end as usize - 1);
            prop_assert!(read(&deleted).is_err(), "closer deleted:\n{}", deleted);
        }

        let mut strings = Vec::new();
        string_spans(&parsed, &mut strings);
        let position = at.index(source.len() + 1);
        let inside_string = strings
            .iter()
            .any(|s| position as u32 > s.start && (position as u32) < s.end);
        if !inside_string && source.is_char_boundary(position) {
            let line_start = source[..position].rfind('\n').map_or(0, |at| at + 1);
            let inside_comment = source[line_start..position].contains(';');
            if !inside_comment {
                let mut inserted = source.clone();
                inserted.insert(position, closer);
                prop_assert!(read(&inserted).is_err(), "closer inserted:\n{}", inserted);
            }
        }
    }

    /// A group read at a base reads as `read` reads the group's own text on a line of its own,
    /// every span shifted by the base, and nothing past the group's closer is read.
    #[test]
    fn read_group_at_reads_one_group_as_read_does(
        rest in "[ ()\\[\\]{},'\";\\na-z#:\\\\]{0,40}",
        base in 0u32..1000,
    ) {
        let text = format!("{{{rest}");
        match (crate::read_group_at(&text, base), crate::read_group_at(&text, 0)) {
            (Ok(at_base), Ok(mut group)) => {
                let end = group.span.end as usize;
                // The group's own text reads as a line holding just the group.
                let line = read(&text[..end]).unwrap_or_else(|e| panic!("{text:?}: {e}"));
                let [Item { node: Node::Group { items, .. }, .. }] = &line[..] else {
                    panic!("{text:?} read as more than one line")
                };
                prop_assert_eq!(&items[..], std::slice::from_ref(&group));
                // Text past the closer is never read, even text that would not read.
                let tail = format!("{}' ) ]", &text[..end]);
                prop_assert_eq!(crate::read_group_at(&tail, 0), Ok(group.clone()));
                shifted(std::slice::from_mut(&mut group), base);
                prop_assert_eq!(at_base, group);
            }
            (Err(at_base), Err(error)) => prop_assert_eq!(
                at_base.span,
                Span { start: error.span.start + base, end: error.span.end + base }
            ),
            (at_base, at_zero) => prop_assert!(false, "{:?} against {:?}", at_base, at_zero),
        }
    }

    /// Arbitrary text never panics the reader.
    #[test]
    fn arbitrary_text_never_panics(source in "[ ()\\[\\]{},'\";\\n\\ta-z#:\\\\]{0,40}") {
        let _ = read(&source);
        let _ = crate::read_group_at(&format!("{{{source}"), 0);
    }
}
