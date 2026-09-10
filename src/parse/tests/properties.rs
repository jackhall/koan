//! The parser's laws, stated over random trees rendered to source under a random layout.
//!
//! A [`Tree`] is a generated koan expression: the surface forms a parse produces a part from,
//! drawn from the three token classes and constrained to what the surface admits. The renderer
//! writes source for one — how many spaces separate two items, whether a trailing group becomes an
//! indented child line, where a comma the surface ignores is sprinkled, and how many redundant
//! `(…)` layers wrap a group are the choices a [`Tape`] of random bytes makes — and
//! [`expected_program`] writes the same tree in the harness's [`describe`](super::describe)
//! notation. Each law reads a rendered tree back and compares against that oracle, so it is stated
//! once rather than pinned one input at a time; the pins that fix a diagnostic message or a
//! surface rule stay in the sibling files.
//!
//! Two facts shape the generator. A keyword is drawn from a pool disjoint from the surface
//! keywords [`FORMS`](crate::parse::forms::FORMS) spells, so no generated run matches a builtin
//! form and the bare-parenthesized type-slot flip never fires. And a `:(…)` body of exactly one
//! sub-expression is re-labelled rather than re-wrapped, so a lone group is never a type sigil's
//! whole body.

use std::collections::BTreeSet;

use proptest::prelude::*;

use super::super::atom::classify_token;
use super::{top, tree};
use crate::machine::core::bindings::powerset_probes;
use crate::memory::program_storage;
use crate::parse::labels::{Symbol, is_keyword_token, is_type_name};
use crate::parse::{
    DispatchShape, ExpressionPart, KExpression, KLiteral, KeyElement, KeywordSymbol, LabelInterner,
    parse,
};
use crate::source::Span;

// --- The generated tree ---

/// A koan expression as the generator builds it: one variant per surface form a parse turns into
/// a part. A `Group` is a `(…)`, a `Quote` a `#(…)`, an `Eval` a `$(…)`, and `Dict` / `Record`
/// are the two readings of a `{…}` its first pair separator selects.
#[derive(Debug, Clone, PartialEq)]
enum Tree {
    Keyword(String),
    Identifier(String),
    Type(String),
    Number(f64),
    Str(String),
    Boolean(bool),
    Null,
    Group(Vec<Tree>),
    TypeSigil(Vec<Tree>),
    RecordType(Vec<(Tree, Tree)>),
    Quote(Vec<Tree>),
    Eval(Vec<Tree>),
    List(Vec<Tree>),
    Dict(Vec<(Tree, Tree)>),
    Record(Vec<(String, Tree)>),
}

/// Whether a redundant `(…)` layer around a group is invisible. A value expression peels it, a
/// type expression keeps it, and the mode carries into everything a type expression nests — so
/// the renderer and the oracle thread the same flag.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Mode {
    Peel,
    Keep,
}

/// A statement is a run of parts; a program is a run of statements.
type Statement = Vec<Tree>;

// --- Generators ---

/// Keyword spellings no builtin form's bucket key names, so a generated run is always a user
/// shape: an alphabetic keyword needs two uppercase letters and no lowercase, and a pure-symbol
/// token is a keyword whatever it spells.
const KEYWORD_POOL: &[&str] = &["ZZ", "QQ", "WW", "+", "*", "<", ">", "|"];

/// Whether `text` reaches [`classify_token`] as a part `want` accepts. Every generated token
/// passes through here, so a spelling the classifier rejects never reaches the renderer.
fn classifies(text: &str, want: fn(&ExpressionPart<'_>) -> bool) -> bool {
    let program = program_storage();
    let labels = LabelInterner::new();
    matches!(
        classify_token(program.brand(), &labels, text, 0),
        Ok(part) if want(&part.value)
    )
}

fn keyword() -> impl Strategy<Value = String> {
    prop::sample::select(KEYWORD_POOL)
        .prop_map(String::from)
        .prop_filter("classifies as a keyword", |text| {
            classifies(text, |part| matches!(part, ExpressionPart::Keyword(_)))
        })
}

fn identifier() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,4}".prop_filter("classifies as an identifier", |text| {
        classifies(text, |part| matches!(part, ExpressionPart::Identifier(_)))
    })
}

fn type_name() -> impl Strategy<Value = String> {
    "[A-Z][a-z][A-Za-z0-9]{0,3}".prop_filter("classifies as a type name", |text| {
        classifies(text, |part| matches!(part, ExpressionPart::Type(_)))
    })
}

/// A leaf that is one token and one part. Non-ASCII string bodies are in the alphabet so a span
/// covers bytes rather than codepoints.
fn scalar() -> BoxedStrategy<Tree> {
    prop_oneof![
        4 => identifier().prop_map(Tree::Identifier),
        3 => type_name().prop_map(Tree::Type),
        2 => (-99i32..1000).prop_map(|n| Tree::Number(f64::from(n))),
        2 => "[a-zé ]{0,4}".prop_map(Tree::Str),
        1 => any::<bool>().prop_map(Tree::Boolean),
        1 => Just(Tree::Null),
    ]
    .boxed()
}

/// A binder name: what a record field key classifies as, on either side of the token partition.
fn binder_name() -> impl Strategy<Value = String> {
    prop_oneof![identifier(), type_name()]
}

/// An element of a list, dict or record literal. A literal holds values, so a bare keyword is
/// refused there — one nested inside a `(…)` is an ordinary element again.
fn literal_tree(depth: u32) -> BoxedStrategy<Tree> {
    if depth == 0 {
        return scalar();
    }
    prop_oneof![
        6 => scalar(),
        2 => prop::collection::vec(value_tree(depth - 1), 0..3).prop_map(Tree::Group),
        1 => prop::collection::vec(literal_tree(depth - 1), 0..3).prop_map(Tree::List),
        1 => dict_pairs(depth - 1).prop_map(Tree::Dict),
        1 => record_fields(depth - 1).prop_map(Tree::Record),
        1 => prop::collection::vec(value_tree(depth - 1), 1..3).prop_map(Tree::Quote),
    ]
    .boxed()
}

fn dict_pairs(depth: u32) -> impl Strategy<Value = Vec<(Tree, Tree)>> {
    prop::collection::vec((scalar(), literal_tree(depth)), 1..3)
}

/// Record fields, deduplicated by name: a repeated field is a diagnostic, not a shape.
fn record_fields(depth: u32) -> impl Strategy<Value = Vec<(String, Tree)>> {
    prop::collection::vec((binder_name(), literal_tree(depth)), 0..3).prop_map(|fields| {
        let mut seen = BTreeSet::new();
        fields
            .into_iter()
            .filter(|(name, _)| seen.insert(name.clone()))
            .collect()
    })
}

/// A `:(…)` body. A body of exactly one sub-expression is re-labelled onto that node rather than
/// wrapped, so a filler part joins a lone group, type sigil or eval.
fn type_sigil_body(depth: u32) -> impl Strategy<Value = Vec<Tree>> {
    prop::collection::vec(value_tree(depth), 0..3).prop_map(|mut items| {
        if matches!(
            items.as_slice(),
            [Tree::Group(_) | Tree::TypeSigil(_) | Tree::Eval(_)]
        ) {
            items.push(Tree::Type("Filler".to_string()));
        }
        items
    })
}

/// A `:{…}` field list: the `<name> :<Type>` pair shape, which lowers to a plain run of parts.
fn record_type_pairs(depth: u32) -> impl Strategy<Value = Vec<(Tree, Tree)>> {
    let annotation = prop_oneof![
        3 => type_name().prop_map(Tree::Type),
        1 => type_sigil_body(depth).prop_map(Tree::TypeSigil),
    ];
    prop::collection::vec(
        (
            prop_oneof![
                identifier().prop_map(Tree::Identifier),
                type_name().prop_map(Tree::Type)
            ],
            annotation,
        ),
        1..3,
    )
}

/// An item of an expression run — everything the surface admits where a keyword is a keyword.
fn value_tree(depth: u32) -> BoxedStrategy<Tree> {
    if depth == 0 {
        return prop_oneof![4 => scalar(), 1 => keyword().prop_map(Tree::Keyword)].boxed();
    }
    prop_oneof![
        6 => scalar(),
        2 => keyword().prop_map(Tree::Keyword),
        3 => prop::collection::vec(value_tree(depth - 1), 0..3).prop_map(Tree::Group),
        1 => type_sigil_body(depth - 1).prop_map(Tree::TypeSigil),
        1 => record_type_pairs(depth - 1).prop_map(Tree::RecordType),
        1 => prop::collection::vec(value_tree(depth - 1), 1..3).prop_map(Tree::Quote),
        1 => prop::collection::vec(value_tree(depth - 1), 1..3).prop_map(Tree::Eval),
        1 => prop::collection::vec(literal_tree(depth - 1), 0..3).prop_map(Tree::List),
        1 => dict_pairs(depth - 1).prop_map(Tree::Dict),
        1 => record_fields(depth - 1).prop_map(Tree::Record),
    ]
    .boxed()
}

/// A statement. A line that leads with `#` or `$` quotes or evaluates the whole line, so a sigil
/// leads a statement only when it is the statement.
fn statement() -> impl Strategy<Value = Statement> {
    prop::collection::vec(value_tree(2), 1..4).prop_map(|mut items| {
        if items.len() > 1 && matches!(items[0], Tree::Quote(_) | Tree::Eval(_)) {
            items.insert(0, Tree::Identifier("head".to_string()));
        }
        items
    })
}

fn program() -> impl Strategy<Value = Vec<Statement>> {
    prop::collection::vec(statement(), 0..3)
}

fn tape_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..96)
}

// --- Renderer ---

/// A tape of layout choices. Each `pick` consumes one byte; an exhausted tape answers `0`, which
/// is always the canonical choice, so shrinking a tape shrinks toward the canonical layout.
struct Tape {
    bytes: Vec<u8>,
    at: usize,
}

impl Tape {
    fn new(bytes: Vec<u8>) -> Self {
        Tape { bytes, at: 0 }
    }

    fn pick(&mut self, n: usize) -> usize {
        let byte = self.bytes.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        byte as usize % n
    }
}

/// Whether the renderer writes the commas it picks. The pick is consumed either way, so the same
/// tape lays a tree out identically apart from the separators.
#[derive(Copy, Clone)]
struct Layout {
    commas: bool,
}

fn spaces(out: &mut String, n: usize) {
    out.extend(std::iter::repeat_n(' ', n));
}

/// Blank lines and trailing whitespace between lines are layout noise.
fn blank_lines(out: &mut String, tape: &mut Tape) {
    match tape.pick(4) {
        1 => out.push('\n'),
        2 => out.push_str("   \n"),
        _ => {}
    }
}

fn render_program(statements: &[Statement], layout: Layout, tape: &mut Tape) -> String {
    let mut out = String::new();
    for statement in statements {
        blank_lines(&mut out, tape);
        render_line(statement, 0, layout, tape, &mut out);
    }
    out
}

/// Whether a trailing item may be written as an indented child line rather than inline. A group
/// leading with a sigil would read as a sigil-led line, and an empty one as a blank line.
fn child_eligible(item: &Tree) -> bool {
    matches!(item, Tree::Group(inner)
        if !inner.is_empty() && !matches!(inner[0], Tree::Quote(_) | Tree::Eval(_)))
}

/// One layout line: an inline run, then a trailing run of groups written as its child lines. The
/// line keeps at least one inline item of its own, and every child of one line indents by the
/// same step.
fn render_line(items: &[Tree], indent: usize, layout: Layout, tape: &mut Tape, out: &mut String) {
    let mut first_child = items.len();
    while first_child > 1 && child_eligible(&items[first_child - 1]) {
        first_child -= 1;
    }
    let children = tape.pick(items.len() - first_child + 1);
    let first_child = items.len() - children;

    spaces(out, indent);
    render_run(&items[..first_child], Mode::Peel, layout, tape, out);
    if tape.pick(3) == 1 {
        out.push_str("  ");
    }
    out.push('\n');
    let step = 2 + 2 * tape.pick(2);
    for child in &items[first_child..] {
        let Tree::Group(inner) = child else {
            unreachable!("only an eligible group becomes a child line")
        };
        blank_lines(out, tape);
        render_line(inner, indent + step, layout, tape, out);
    }
}

fn render_run(items: &[Tree], mode: Mode, layout: Layout, tape: &mut Tape, out: &mut String) {
    for (index, item) in items.iter().enumerate() {
        render_item(item, mode, layout, tape, out);
        if index + 1 == items.len() {
            continue;
        }
        // A comma is whitespace outside a brace, so it may sit between any two items. It never
        // ends a line, where it would join the next one instead.
        if tape.pick(3) == 1 && layout.commas {
            out.push_str(" ,");
        }
        spaces(out, 1 + tape.pick(3));
    }
}

fn render_item(item: &Tree, mode: Mode, layout: Layout, tape: &mut Tape, out: &mut String) {
    match item {
        Tree::Keyword(text) | Tree::Identifier(text) => out.push_str(text),
        // A type name reaches a type position either bare or through a glued `:`.
        Tree::Type(text) => {
            if tape.pick(2) == 1 {
                out.push(':');
            }
            out.push_str(text);
        }
        Tree::Number(value) => out.push_str(&value.to_string()),
        Tree::Str(body) => {
            out.push('\'');
            out.push_str(body);
            out.push('\'');
        }
        Tree::Boolean(value) => out.push_str(if *value { "true" } else { "false" }),
        Tree::Null => out.push_str("null"),
        Tree::Group(items) => {
            let redundant = if mode == Mode::Peel { tape.pick(3) } else { 0 };
            for _ in 0..=redundant {
                out.push('(');
            }
            render_run(items, mode, layout, tape, out);
            for _ in 0..=redundant {
                out.push(')');
            }
        }
        Tree::TypeSigil(items) => {
            out.push_str(":(");
            render_run(items, Mode::Keep, layout, tape, out);
            out.push(')');
        }
        Tree::RecordType(pairs) => {
            out.push_str(":{");
            render_run(&flatten(pairs), Mode::Keep, layout, tape, out);
            out.push('}');
        }
        Tree::Quote(items) => {
            out.push_str("#(");
            render_run(items, mode, layout, tape, out);
            out.push(')');
        }
        Tree::Eval(items) => {
            out.push_str("$(");
            render_run(items, mode, layout, tape, out);
            out.push(')');
        }
        Tree::List(items) => {
            out.push('[');
            render_run(items, mode, layout, tape, out);
            if !items.is_empty() && tape.pick(3) == 1 && layout.commas {
                out.push_str(" ,");
            }
            out.push(']');
        }
        Tree::Dict(pairs) => {
            out.push('{');
            for (index, (key, value)) in pairs.iter().enumerate() {
                render_key(key, out);
                spaces(out, 1 + tape.pick(3));
                render_item(value, mode, layout, tape, out);
                if index + 1 < pairs.len() {
                    // A pair commits on its own when the next key arrives, so the comma is a
                    // separator the surface admits rather than one it needs.
                    if tape.pick(2) == 1 {
                        out.push_str(" ,");
                    }
                    spaces(out, 1 + tape.pick(3));
                }
            }
            if tape.pick(3) == 1 {
                out.push_str(" ,");
            }
            out.push('}');
        }
        Tree::Record(fields) => {
            out.push('{');
            for (index, (name, value)) in fields.iter().enumerate() {
                out.push_str(name);
                spaces(out, 1 + tape.pick(3));
                out.push('=');
                spaces(out, 1 + tape.pick(3));
                render_item(value, mode, layout, tape, out);
                if index + 1 < fields.len() {
                    if tape.pick(2) == 1 {
                        out.push_str(" ,");
                    }
                    spaces(out, 1 + tape.pick(3));
                }
            }
            if !fields.is_empty() && tape.pick(3) == 1 {
                out.push_str(" ,");
            }
            out.push('}');
        }
    }
}

/// A dict key and the `:` that pairs it. The colon is glued to the key, and a space follows it,
/// so it never reads as the type sigil.
fn render_key(key: &Tree, out: &mut String) {
    match key {
        Tree::Keyword(text) | Tree::Identifier(text) | Tree::Type(text) => out.push_str(text),
        Tree::Number(value) => out.push_str(&value.to_string()),
        Tree::Str(body) => {
            out.push('\'');
            out.push_str(body);
            out.push('\'');
        }
        Tree::Boolean(value) => out.push_str(if *value { "true" } else { "false" }),
        Tree::Null => out.push_str("null"),
        other => unreachable!("a dict key is a scalar, got {other:?}"),
    }
    out.push(':');
}

fn flatten(pairs: &[(Tree, Tree)]) -> Vec<Tree> {
    pairs
        .iter()
        .flat_map(|(left, right)| [left.clone(), right.clone()])
        .collect()
}

// --- The oracle ---

fn expected_program(statements: &[Statement]) -> Vec<String> {
    statements.iter().map(|s| expected_statement(s)).collect()
}

fn expected_statement(items: &[Tree]) -> String {
    body_describe(items, Mode::Peel)
}

/// A run that is exactly one group names what the group names, however many layers deep.
fn peeled(items: &[Tree]) -> &[Tree] {
    let mut items = items;
    while let [Tree::Group(inner)] = items {
        items = inner;
    }
    items
}

/// A node's own `[…]` rendering. Where a redundant wrapper is invisible, a run that is exactly
/// one group names what the group names, and a run that is exactly one sub-expression *is* that
/// expression — which is what a `$(…)` lowers to, so an eval sigil alone names the `EVAL` call it
/// asks for rather than a node holding it.
fn body_describe(items: &[Tree], mode: Mode) -> String {
    if mode == Mode::Keep {
        return format!("[{}]", run_describe(items, mode));
    }
    let items = peeled(items);
    if let [Tree::Eval(body)] = items {
        return format!("[t(EVAL) {}]", body_describe(body, Mode::Peel));
    }
    format!("[{}]", run_describe(items, Mode::Peel))
}

fn run_describe(items: &[Tree], mode: Mode) -> String {
    items
        .iter()
        .map(|item| describe_tree(item, mode))
        .collect::<Vec<_>>()
        .join(" ")
}

fn describe_tree(item: &Tree, mode: Mode) -> String {
    match item {
        Tree::Keyword(text) | Tree::Identifier(text) => format!("t({text})"),
        Tree::Type(text) => format!("T({text})"),
        Tree::Number(value) => format!("n({value})"),
        Tree::Str(body) => format!("s({body})"),
        Tree::Boolean(value) => format!("b({value})"),
        Tree::Null => "null".to_string(),
        Tree::Group(items) => body_describe(items, mode),
        Tree::TypeSigil(items) => format!(":({})", run_describe(items, Mode::Keep)),
        Tree::RecordType(pairs) => {
            format!(":{{{}}}", run_describe(&flatten(pairs), Mode::Keep))
        }
        Tree::Quote(items) => format!("#{}", body_describe(items, mode)),
        Tree::Eval(items) => format!("[t(EVAL) {}]", body_describe(items, mode)),
        Tree::List(items) => format!("L[{}]", run_describe(items, mode)),
        Tree::Dict(pairs) => format!(
            "D{{{}}}",
            pairs
                .iter()
                .map(|(key, value)| format!(
                    "{}: {}",
                    describe_tree(key, mode),
                    describe_tree(value, mode)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Tree::Record(fields) => format!(
            "R{{{}}}",
            fields
                .iter()
                .map(|(name, value)| format!("{name} = {}", describe_tree(value, mode)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

// --- Readers the laws share ---

const COMMAS: Layout = Layout { commas: true };

fn render(statements: &[Statement], bytes: Vec<u8>) -> String {
    render_program(statements, COMMAS, &mut Tape::new(bytes))
}

fn read_back(source: &str) -> Vec<String> {
    top(source).unwrap_or_else(|error| panic!("{source:?}: {error}"))
}

/// What a statement carries beyond its shape string: the cache a node fills at construction, and
/// the extent it claims in the source.
struct StatementFacts {
    shape_string: String,
    key: Vec<KeyElement>,
    shape: DispatchShape,
    operator_probe: Option<KeywordSymbol>,
    span: Option<Span>,
}

fn statement_facts(source: &str) -> StatementFacts {
    let program = program_storage();
    let labels = LabelInterner::new();
    let statements =
        parse(program.brand(), &labels, source).unwrap_or_else(|e| panic!("{source:?}: {e}"));
    let [statement] = statements.as_slice() else {
        panic!("{source:?}: expected exactly one statement")
    };
    StatementFacts {
        shape_string: super::describe(statement, &labels),
        key: statement.stored_key().to_vec(),
        shape: statement.shape(),
        operator_probe: statement.operator_probe(),
        span: statement.span,
    }
}

// --- Span walk (law 4) ---

fn within(span: Span, parent: Option<Span>) {
    if let Some(parent) = parent {
        assert!(
            span.start >= parent.start && span.end <= parent.end,
            "a child span lies within its parent",
        );
    }
}

fn check_expression(
    expr: &KExpression<'_>,
    source: &str,
    labels: &LabelInterner,
    parent: Option<Span>,
) {
    let span = expr.span.expect("a parsed node carries its span");
    within(span, parent);
    let mut previous_end = span.start;
    for part in expr.parts {
        let part_span = part.span.expect("a parsed part carries its span");
        assert!(
            part_span.start >= previous_end,
            "siblings are ordered and disjoint",
        );
        within(part_span, Some(span));
        check_part(&part.value, part_span, source, labels);
        previous_end = part_span.end;
    }
}

fn check_part(part: &ExpressionPart<'_>, span: Span, source: &str, labels: &LabelInterner) {
    let text = span.slice(source);
    match part {
        ExpressionPart::Keyword(symbol) => {
            let rendering = labels.render(symbol.symbol());
            // A synthetic operator head takes its trigger's span instead: the `$` an evaluation
            // was written as, the `.` or `?` a compound atom folded on.
            if rendering != text {
                assert!(
                    matches!(rendering.as_str(), "EVAL" | "ATTR" | "TRY"),
                    "a keyword slices back to its spelling, got {text:?} for {rendering:?}",
                );
                assert_eq!(
                    text.chars().count(),
                    1,
                    "a synthetic head spans its trigger"
                );
            }
        }
        ExpressionPart::Identifier(name) => assert_eq!(labels.render(name.symbol()), text),
        ExpressionPart::Type(name) => assert_eq!(labels.render(name.symbol()), text),
        ExpressionPart::Literal(KLiteral::Number(value)) => {
            assert_eq!(text.parse::<f64>().ok(), Some(*value));
        }
        ExpressionPart::Literal(KLiteral::String(body)) => {
            assert_eq!(text, format!("'{body}'"));
        }
        ExpressionPart::Literal(KLiteral::Boolean(value)) => assert_eq!(text, value.to_string()),
        ExpressionPart::Literal(KLiteral::Null) => assert_eq!(text, "null"),
        ExpressionPart::Expression(node) => check_expression(node, source, labels, Some(span)),
        ExpressionPart::SigiledTypeExpr(node) => {
            assert!(text.starts_with(':') && text.ends_with(')'), "got {text:?}");
            check_expression(node, source, labels, Some(span));
        }
        ExpressionPart::RecordType(node) => {
            assert!(
                text.starts_with(":{") && text.ends_with('}'),
                "got {text:?}"
            );
            check_expression(node, source, labels, Some(span));
        }
        ExpressionPart::QuotedExpression(node) => {
            assert!(text.starts_with('#') && text.ends_with(')'), "got {text:?}");
            check_expression(node, source, labels, Some(span));
        }
        ExpressionPart::ListLiteral(items) => {
            assert!(text.starts_with('[') && text.ends_with(']'), "got {text:?}");
            for item in *items {
                check_nested(item, span, source, labels);
            }
        }
        ExpressionPart::DictLiteral(pairs) => {
            assert!(text.starts_with('{') && text.ends_with('}'), "got {text:?}");
            for (key, value) in *pairs {
                check_nested(key, span, source, labels);
                check_nested(value, span, source, labels);
            }
        }
        ExpressionPart::RecordLiteral(fields) => {
            assert!(text.starts_with('{') && text.ends_with('}'), "got {text:?}");
            for (_, value) in *fields {
                check_nested(value, span, source, labels);
            }
        }
    }
}

/// A literal's elements carry no span of their own; the nodes among them still do.
fn check_nested(part: &ExpressionPart<'_>, parent: Span, source: &str, labels: &LabelInterner) {
    match part {
        ExpressionPart::Expression(node)
        | ExpressionPart::SigiledTypeExpr(node)
        | ExpressionPart::RecordType(node)
        | ExpressionPart::QuotedExpression(node) => {
            check_expression(node, source, labels, Some(parent));
        }
        _ => {}
    }
}

// --- Symbol walk (law 7) ---

fn tree_spellings(item: &Tree, out: &mut BTreeSet<String>) {
    match item {
        Tree::Keyword(text) | Tree::Identifier(text) | Tree::Type(text) => {
            out.insert(text.clone());
        }
        Tree::Number(_) | Tree::Str(_) | Tree::Boolean(_) | Tree::Null => {}
        Tree::Group(items) | Tree::Quote(items) | Tree::TypeSigil(items) | Tree::List(items) => {
            for item in items {
                tree_spellings(item, out);
            }
        }
        // Evaluation is a runtime operation, so a `$(…)` mints the `EVAL` head it calls.
        Tree::Eval(items) => {
            out.insert("EVAL".to_string());
            for item in items {
                tree_spellings(item, out);
            }
        }
        Tree::RecordType(pairs) | Tree::Dict(pairs) => {
            for (left, right) in pairs {
                tree_spellings(left, out);
                tree_spellings(right, out);
            }
        }
        Tree::Record(fields) => {
            for (name, value) in fields {
                out.insert(name.clone());
                tree_spellings(value, out);
            }
        }
    }
}

fn node_symbols(expr: &KExpression<'_>, out: &mut Vec<Symbol>) {
    for part in expr.parts {
        part_symbols(&part.value, out);
    }
}

fn part_symbols(part: &ExpressionPart<'_>, out: &mut Vec<Symbol>) {
    match part {
        ExpressionPart::Keyword(symbol) => out.push(symbol.symbol()),
        ExpressionPart::Identifier(name) => out.push(name.symbol()),
        ExpressionPart::Type(name) => out.push(name.symbol()),
        ExpressionPart::Expression(node)
        | ExpressionPart::SigiledTypeExpr(node)
        | ExpressionPart::RecordType(node)
        | ExpressionPart::QuotedExpression(node) => node_symbols(node, out),
        ExpressionPart::ListLiteral(items) => {
            for item in *items {
                part_symbols(item, out);
            }
        }
        ExpressionPart::DictLiteral(pairs) => {
            for (key, value) in *pairs {
                part_symbols(key, out);
                part_symbols(value, out);
            }
        }
        ExpressionPart::RecordLiteral(fields) => {
            for (name, value) in *fields {
                out.push(name.symbol());
                part_symbols(value, out);
            }
        }
        ExpressionPart::Literal(_) => {}
    }
}

// --- Token classification (law 3) ---

/// The class a token text belongs to, read off the published predicates in the order the
/// classifier applies them: a literal spelling first, then the keyword / Type / value partition.
#[derive(Debug, PartialEq, Eq)]
enum TokenClass {
    Literal,
    Keyword,
    Type,
    Identifier,
}

fn reference_class(text: &str) -> TokenClass {
    if matches!(text, "null" | "true" | "false")
        || (number_shape(text) && text.parse::<f64>().is_ok())
    {
        return TokenClass::Literal;
    }
    if is_keyword_token(text) {
        return TokenClass::Keyword;
    }
    if is_type_name(text) {
        return TokenClass::Type;
    }
    TokenClass::Identifier
}

/// koan's decimal-number shape: an optional sign, digits with an optional point carrying at least
/// one digit on some side, and an optional exponent. Written out here so the law reads the
/// classifier against an independent statement of the grammar.
fn number_shape(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut at = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        at = 1;
    }
    let integral = digits(bytes, &mut at);
    let mut fractional = 0;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        fractional = digits(bytes, &mut at);
    }
    if integral == 0 && fractional == 0 {
        return false;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        if digits(bytes, &mut at) == 0 {
            return false;
        }
    }
    at == bytes.len()
}

fn digits(bytes: &[u8], at: &mut usize) -> usize {
    let start = *at;
    while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
        *at += 1;
    }
    *at - start
}

// --- Compound desugaring (law 10) ---

/// One step of a compound atom: `.name` reads a field, `?` is the try suffix.
#[derive(Debug, Clone)]
enum Suffix {
    Attr { name: String, is_type: bool },
    Try,
}

fn suffix() -> impl Strategy<Value = Suffix> {
    prop_oneof![
        2 => identifier().prop_map(|name| Suffix::Attr { name, is_type: false }),
        2 => type_name().prop_map(|name| Suffix::Attr { name, is_type: true }),
        1 => Just(Suffix::Try),
    ]
}

/// The token a base plus its suffixes spells, and the part that token classifies to. A field
/// whose name is a Type token makes the access a type operation, which the sigiled arm carries.
fn compound(base: &str, base_shape: &str, suffixes: &[Suffix]) -> (String, String) {
    let mut token = base.to_string();
    let mut shape = base_shape.to_string();
    for step in suffixes {
        match step {
            Suffix::Attr {
                name,
                is_type: false,
            } => {
                token.push('.');
                token.push_str(name);
                shape = format!("[t(ATTR) {shape} t({name})]");
            }
            Suffix::Attr {
                name,
                is_type: true,
            } => {
                token.push('.');
                token.push_str(name);
                shape = format!(":(t(ATTR) {shape} T({name}))");
            }
            Suffix::Try => {
                token.push('?');
                shape = format!("[t(TRY) {shape}]");
            }
        }
    }
    (token, shape)
}

// --- The laws ---

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// **Law 1.** Any layout the surface admits reads back as the tree it was rendered from:
    /// how wide a gap separates two items, whether a trailing group is written inline or as an
    /// indented child line, where a comma the surface ignores falls, and how many redundant
    /// `(…)` layers wrap a group are all layout, not structure.
    #[test]
    fn a_rendered_tree_reads_back_as_itself(statements in program(), bytes in tape_bytes()) {
        let source = render(&statements, bytes);
        prop_assert_eq!(read_back(&source), expected_program(&statements), "source:\n{}", source);
    }

    /// **Law 2.** A group that wraps nothing but the statement is redundant: peeling it leaves
    /// the same parts and the same cache, and the survivor claims the outermost wrapper's extent.
    #[test]
    fn a_redundant_wrapper_peels_to_the_same_statement(
        statement in statement(),
        bytes in tape_bytes(),
    ) {
        let mut tape = Tape::new(bytes);
        let mut inner = String::new();
        render_run(&statement, Mode::Peel, COMMAS, &mut tape, &mut inner);
        let bare = statement_facts(&inner);
        for layers in 1..3 {
            let wrapped = format!("{}{inner}{}", "(".repeat(layers), ")".repeat(layers));
            let facts = statement_facts(&wrapped);
            prop_assert_eq!(&facts.shape_string, &bare.shape_string, "source: {}", wrapped);
            prop_assert_eq!(&facts.key, &bare.key);
            prop_assert_eq!(facts.shape, bare.shape);
            prop_assert_eq!(facts.operator_probe, bare.operator_probe);
            prop_assert_eq!(
                facts.span,
                Some(Span { start: 0, end: wrapped.len() as u32 }),
                "the survivor takes the outermost wrapper's span",
            );
        }
    }

    /// **Law 3.** The literal spellings, `is_keyword_token`, `is_type_name` and their complement
    /// name one class per token, and `classify_token` answers exactly that class — or rejects the
    /// token, which it does precisely when the class's spelling rule is broken.
    #[test]
    fn classification_answers_the_one_class_a_token_belongs_to(
        token in "[A-Za-z0-9_:+*<>=|!~^&/@%-]{1,6}",
    ) {
        prop_assert!(
            !(is_keyword_token(&token) && is_type_name(&token)),
            "the keyword and Type classes are disjoint",
        );
        let program = program_storage();
        let labels = LabelInterner::new();
        let classified = classify_token(program.brand(), &labels, &token, 0);
        let part = classified.as_ref().map(|spanned| &spanned.value);
        match reference_class(&token) {
            TokenClass::Literal => {
                prop_assert!(matches!(part, Ok(ExpressionPart::Literal(_))), "{token:?}");
            }
            TokenClass::Keyword => {
                let Ok(ExpressionPart::Keyword(symbol)) = part else {
                    return Err(TestCaseError::fail(format!("{token:?} is keyword-class")));
                };
                prop_assert_eq!(labels.render(symbol.symbol()), token.as_str());
            }
            TokenClass::Type => {
                if token.chars().all(|c| c.is_ascii_alphanumeric()) {
                    let Ok(ExpressionPart::Type(name)) = part else {
                        return Err(TestCaseError::fail(format!("{token:?} is Type-class")));
                    };
                    prop_assert_eq!(labels.render(name.symbol()), token.as_str());
                } else {
                    prop_assert!(part.is_err(), "a Type name uses letters and digits only");
                }
            }
            TokenClass::Identifier => {
                let spellable = !token.starts_with(|c: char| c.is_ascii_uppercase())
                    && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if spellable {
                    let Ok(ExpressionPart::Identifier(name)) = part else {
                        return Err(TestCaseError::fail(format!("{token:?} is a value token")));
                    };
                    prop_assert_eq!(labels.render(name.symbol()), token.as_str());
                } else {
                    prop_assert!(part.is_err(), "{token:?} classifies as no token class");
                }
            }
        }
    }

    /// **Law 4.** Every span indexes its own text: a name slices back to its spelling, a literal
    /// to how it was written, and a delimited part to its delimiters. Children lie within their
    /// parents, and siblings are ordered and disjoint.
    #[test]
    fn every_span_indexes_its_own_text(statements in program(), bytes in tape_bytes()) {
        let source = render(&statements, bytes);
        let program = program_storage();
        let labels = LabelInterner::new();
        let parsed = parse(program.brand(), &labels, &source)
            .unwrap_or_else(|error| panic!("{source:?}: {error}"));
        let mut previous_end = 0;
        for statement in &parsed {
            let span = statement.span.expect("a parsed statement carries its span");
            prop_assert!(span.start >= previous_end, "statements are ordered and disjoint");
            check_expression(statement, &source, &labels, None);
            previous_end = span.end;
        }
    }

    /// **Law 5.** A container holds one entry per item written: `n` list elements parse to `n`
    /// parts, `n` dict pairs to `n`, and `n` distinct record fields to `n`, under any sprinkling
    /// of the separators the surface admits.
    #[test]
    fn a_container_holds_one_entry_per_item(
        container in prop_oneof![
            prop::collection::vec(literal_tree(1), 0..4).prop_map(Tree::List),
            prop::collection::vec((scalar(), literal_tree(1)), 1..4).prop_map(Tree::Dict),
            record_fields(1).prop_map(Tree::Record),
        ],
        bytes in tape_bytes(),
    ) {
        let expected = match &container {
            Tree::List(items) => items.len(),
            Tree::Dict(pairs) => pairs.len(),
            Tree::Record(fields) => fields.len(),
            other => unreachable!("{other:?}"),
        };
        let mut source = String::new();
        render_item(&container, Mode::Peel, COMMAS, &mut Tape::new(bytes), &mut source);
        let program = program_storage();
        let labels = LabelInterner::new();
        let parsed = parse(program.brand(), &labels, &source)
            .unwrap_or_else(|error| panic!("{source:?}: {error}"));
        let [statement] = parsed.as_slice() else {
            return Err(TestCaseError::fail(format!("{source:?}: one statement")));
        };
        let arity = match statement.parts[0].value {
            ExpressionPart::ListLiteral(items) => items.len(),
            ExpressionPart::DictLiteral(pairs) => pairs.len(),
            ExpressionPart::RecordLiteral(fields) => fields.len(),
            other => return Err(TestCaseError::fail(format!("{source:?}: got {other:?}"))),
        };
        prop_assert_eq!(arity, expected, "source: {}", source);
    }

    /// **Law 6.** A comma where the surface ignores it is whitespace: writing one, or leaving it
    /// out, leaves the tree unchanged.
    #[test]
    fn a_separator_the_surface_ignores_changes_nothing(
        statements in program(),
        bytes in tape_bytes(),
    ) {
        let with = render_program(&statements, COMMAS, &mut Tape::new(bytes.clone()));
        let without =
            render_program(&statements, Layout { commas: false }, &mut Tape::new(bytes));
        prop_assert_eq!(read_back(&with), read_back(&without), "source:\n{}", with);
    }

    /// **Law 7.** A part carries a symbol and nothing else, so the parse that classified the
    /// token is what recorded its spelling: every symbol a parse holds resolves, and the table
    /// holds one entry per distinct spelling and no more.
    #[test]
    fn every_symbol_a_parse_mints_is_recorded(statements in program(), bytes in tape_bytes()) {
        let source = render(&statements, bytes);
        let mut spellings = BTreeSet::new();
        for statement in &statements {
            for item in statement {
                tree_spellings(item, &mut spellings);
            }
        }
        let program = program_storage();
        let labels = LabelInterner::new();
        let parsed = parse(program.brand(), &labels, &source)
            .unwrap_or_else(|error| panic!("{source:?}: {error}"));
        let mut symbols = Vec::new();
        for statement in &parsed {
            node_symbols(statement, &mut symbols);
        }
        for symbol in symbols {
            let text = labels.resolve(symbol);
            prop_assert!(text.is_some(), "every minted symbol resolves");
            prop_assert!(spellings.contains(&text.expect("just checked")));
        }
        for spelling in &spellings {
            let recorded = labels.resolve(Symbol::of(spelling));
            prop_assert_eq!(recorded.as_deref(), Some(spelling.as_str()));
        }
        prop_assert_eq!(labels.len(), spellings.len(), "one entry per distinct spelling");
    }

    /// **Law 8.** A chain's probe is the digest of the operator set it names — order-free and
    /// repeat-free — and a group over any superset of those operators registers that very key,
    /// so a live chain finds its group by construction.
    #[test]
    fn a_chains_probe_is_the_digest_of_its_operator_set(
        operands in prop::collection::vec(identifier(), 3..6),
        choices in prop::collection::vec(0usize..4, 5),
        extra in prop::sample::select(&["+", "*", "<", ">"][..]),
    ) {
        let pool = ["+", "*", "<", ">"];
        let operators: Vec<&str> =
            choices[..operands.len() - 1].iter().map(|at| pool[*at]).collect();
        let mut source = operands[0].clone();
        for (operator, operand) in operators.iter().zip(&operands[1..]) {
            source.push_str(&format!(" {operator} {operand}"));
        }

        let program = program_storage();
        let labels = LabelInterner::new();
        let parsed = parse(program.brand(), &labels, &source)
            .unwrap_or_else(|error| panic!("{source:?}: {error}"));
        let statement = &parsed[0];
        prop_assert_eq!(statement.shape(), DispatchShape::OperatorChain, "source: {}", source);

        let mut distinct: Vec<KeywordSymbol> = Vec::new();
        for operator in &operators {
            let symbol = KeywordSymbol::declared(operator, &labels)
                .expect("an operator glyph is keyword-class");
            if !distinct.contains(&symbol) {
                distinct.push(symbol);
            }
        }
        prop_assert_eq!(statement.operator_probe(), Some(KeywordSymbol::of_run(&distinct)));

        let mut members = distinct.clone();
        let extra = KeywordSymbol::declared(extra, &labels).expect("keyword-class");
        if !members.contains(&extra) {
            members.push(extra);
        }
        let installed = powerset_probes(&members, &labels);
        prop_assert!(
            installed.contains(&statement.operator_probe().expect("a chain carries a probe")),
            "a group over a superset registers the key {source:?} probes",
        );
    }

    /// **Law 9.** The type sigil is idempotent: a body wrapped `k` layers deep names what one
    /// layer names, because a sigil over a lone sub-expression re-labels that node instead of
    /// adding a layer. That is also why an attribute access with a Type-classed field — already
    /// a type operation — reads the same bare as it does sigiled.
    #[test]
    fn the_type_sigil_is_idempotent(
        tokens in prop::collection::vec(
            prop_oneof![
                type_name(),
                identifier(),
                (identifier(), identifier()).prop_map(|(a, b)| format!("{a}.{b}")),
                (type_name(), identifier()).prop_map(|(a, b)| format!("{a}.{b}")),
                (type_name(), type_name()).prop_map(|(a, b)| format!("{a}.{b}")),
                (type_name(), identifier()).prop_map(|(a, b)| format!("({a} {b})")),
            ],
            0..3,
        ),
    ) {
        let body = tokens.join(" ");
        let once = read_back(&format!(":({body})"));
        for layers in 2..4 {
            let nested = format!("{}{body}{}", ":(".repeat(layers), ")".repeat(layers));
            prop_assert_eq!(read_back(&nested), once.as_slice(), "source: {}", nested);
        }
        // A Type-classed field makes the access a type operation, so `build_attr` already emits
        // the shape the explicit sigil would wrap.
        if let [token] = tokens.as_slice()
            && let Some((_, field)) = token.split_once('.')
            && is_type_name(field)
        {
            prop_assert_eq!(read_back(token), once.as_slice());
        }
    }

    /// **Law 10.** A whole-token literal match runs first, so a number keeps its spelling; every
    /// other compound atom desugars left-nested, one operator at a time: each `.` builds an
    /// `ATTR` over everything read so far, each `?` a `TRY`, and a Type-classed field wraps the
    /// access as a type expression.
    #[test]
    fn a_compound_atom_desugars_left_nested(
        base in prop_oneof![
            identifier().prop_map(|name| (name.clone(), format!("t({name})"))),
            type_name().prop_map(|name| (name.clone(), format!("T({name})"))),
            (0i32..100).prop_map(|n| (n.to_string(), format!("n({n})"))),
        ],
        suffixes in prop::collection::vec(suffix(), 1..4),
    ) {
        let (token, shape) = compound(&base.0, &base.1, &suffixes);
        let literal = number_shape(&token).then(|| token.parse::<f64>().ok()).flatten();
        let expected = match literal {
            Some(value) => format!("[n({value})]"),
            None => format!("[{shape}]"),
        };
        prop_assert_eq!(
            tree(&token).unwrap_or_else(|error| panic!("{token:?}: {error}")),
            expected,
        );
    }
}
