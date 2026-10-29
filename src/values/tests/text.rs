//! String views: slices of a string, forward or backward, read in code points and compared,
//! rendered, digested, keyed and copied as the flat strings they stand for.

use crate::memory::CellGraph;
use crate::values::text::DECODED;
use crate::values::{Key, Text, ValueFamily};

use super::{Dict, TestGraph, Value, copy, entry, pin, text, with_fixture};

/// The text a string value shows.
fn shown(value: Value<'_>) -> String {
    value.as_text().expect("a string").chars().collect()
}

#[test]
fn a_slice_of_a_string_or_a_view_reads_its_code_points() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let (writer, scratch) = (context.writer(), fixture.scratch());
            for source in ["hello", "héllo", "a😀b€c", ""] {
                let flat = text(writer, source);
                let model: Vec<char> = source.chars().collect();
                let len = model.len();
                assert_eq!(flat.as_text().unwrap().len(), len);
                for from in 0..=len {
                    for to in from..=len {
                        for backward in [false, true] {
                            let slice =
                                flat.as_text()
                                    .unwrap()
                                    .anchored(writer, scratch)
                                    .sliced::<crate::values::Nothing>(writer, (from, to), backward);
                            let mut part = model[from..to].to_vec();
                            if backward {
                                part.reverse();
                            }
                            let expected: String = part.iter().collect();
                            assert_eq!(shown(slice), expected);
                            assert_eq!(slice.as_text().unwrap().len(), part.len());
                            // A slice of the view: every code point but its first, reversed.
                            if part.len() > 1 {
                                let again = slice
                                    .as_text()
                                    .unwrap()
                                    .anchored(writer, scratch)
                                    .sliced::<crate::values::Nothing>(
                                    writer,
                                    (1, part.len()),
                                    true,
                                );
                                let expected: String = part[1..].iter().rev().collect();
                                assert_eq!(shown(again), expected);
                            }
                            for (at, expected) in part.iter().enumerate() {
                                let point = slice
                                    .as_text()
                                    .unwrap()
                                    .code_point::<crate::values::Nothing>(writer, at)
                                    .expect("a code point in range");
                                assert_eq!(point.as_str(), Some(expected.to_string().as_str()));
                            }
                        }
                    }
                }
            }
        })
    });
}

#[test]
fn a_view_of_a_long_string_finds_a_code_point_within_one_crumb() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let (writer, scratch) = (context.writer(), fixture.scratch());
            let source = "é".repeat(1000);
            let flat = text(writer, &source);
            let view = flat
                .as_text()
                .unwrap()
                .anchored(writer, scratch)
                .sliced::<crate::values::Nothing>(writer, (5, 990), true);
            let view = view.as_text().unwrap();
            for at in [0, 63, 64, 500, 984] {
                let before = DECODED.with(|decoded| decoded.get());
                let point = view
                    .code_point::<crate::values::Nothing>(writer, at)
                    .expect("a code point in range");
                assert_eq!(point.as_str(), Some("é"));
                let decoded = DECODED.with(|decoded| decoded.get()) - before;
                assert!(
                    decoded < 64,
                    "a read decoded {decoded} code points past its crumb"
                );
            }
        })
    });
}

#[test]
fn a_string_view_equals_renders_digests_and_keys_as_its_flat_string() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let (scratch, symbols) = (fixture.scratch(), fixture.symbols);
            let source = text(writer, "olé!");
            let view =
                source
                    .as_text()
                    .unwrap()
                    .anchored(writer, scratch)
                    .sliced(writer, (0, 4), true);
            let flat = text(writer, "!élo");
            assert_eq!(view.equals(&flat, types, scratch), Ok(true));
            assert_eq!(view.equals(&source, types, scratch), Ok(false));
            let render = |value: Value<'_>| {
                let mut out = String::new();
                value.render(&mut out, types, symbols, scratch).unwrap();
                out
            };
            assert_eq!(render(view), "!élo");
            assert_eq!(view.digest(types, scratch), flat.digest(types, scratch));
            let forward: Value<'_> =
                source
                    .as_text()
                    .unwrap()
                    .anchored(writer, scratch)
                    .sliced(writer, (1, 3), false);
            assert_eq!(
                forward.digest(types, scratch),
                text(writer, "lé").digest(types, scratch)
            );
            assert!(view.same_storage(&view));
            assert!(!view.same_storage(&flat));

            let entries = [(Key::str("!élo"), Value::Number(1.0))];
            let dict = Dict::new(writer, &entries, types, scratch);
            let key = Key::of(&view, writer).unwrap();
            assert_eq!(
                entry(dict, &key).and_then(|value| match value {
                    Value::Number(number) => Some(*number),
                    _ => None,
                }),
                Some(1.0)
            );
            assert_eq!(Key::admits(&view), Ok(()));
            assert!(matches!(view.as_text(), Some(Text::View(_))));
        })
    });
}

#[test]
fn a_copied_string_view_is_a_flat_string() {
    with_fixture(|fixture| {
        let mut graph: TestGraph<'_> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        graph
            .enter(home, |context| {
                let (writer, scratch) = (context.writer(), fixture.scratch());
                let source = text(writer, "a long string, of which a little is kept");
                for backward in [false, true] {
                    let view = source.as_text().unwrap().anchored(writer, scratch).sliced(
                        writer,
                        (2, 6),
                        backward,
                    );
                    let carrier = context.lift::<ValueFamily>(view);
                    let crossed = fixture.tiers.cross(context, dest, &carrier).unwrap();
                    let copied = context.read(&crossed).value();
                    let expected = if backward { "gnol" } else { "long" };
                    assert_eq!(copied.as_str(), Some(expected));
                }
            })
            .unwrap();
    });
}
