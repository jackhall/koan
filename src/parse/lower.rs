//! Lowering: the layout tree [`sexlex`] read from the source becomes one `KExpression` per
//! top-level line. This is where koan's vocabulary enters — which atoms are sigils, what a colon
//! binds to, what a comma means inside a brace — none of which the layout pass knows.
//!
//! Six rules cover it. A **run** of sibling items becomes a run of parts ([`Lower::lower_run`]),
//! with the context (expression, list element, brace entry) deciding where each part lands. A
//! **body** is a run that becomes a node ([`Lower::lower_body`]), and is where a redundant wrapper
//! is peeled: `((a b))` and `(a b)` name the same expression, and so does the group a body line
//! nests in. A **sigil** is `#`, `$` or `:` glued to the group after it, and a **sigil-led line**
//! is a whole layout line whose first atom starts with `#` or `$` — the line's own body is what it
//! quotes. **Adjacency** rejects a `[` or `{` glued to a neighbouring token. Everything else is an
//! atom, which [`super::atom`] classifies.
//!
//! See [design/expressions-and-parsing.md](../../design/expressions-and-parsing.md).

use sexlex::{Item, Kind, Node};

use crate::machine::KError;
use crate::machine::model::admit_bare_type_slots;
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral, ProgramExpression};
use crate::machine::model::labels::{KeywordSymbol, LabelInterner};
use crate::memory::ProgramBrand;
use crate::source::{FileId, Span, Spanned};

use super::atom;
use super::brace::{BraceContents, DictFrame};

/// Read `source` into its layout tree and lower every top-level line. `file` is read once by the
/// caller and stamped on every node this produces.
pub(super) fn lower_source<'a>(
    program: ProgramBrand<'a>,
    labels: &LabelInterner,
    source: &str,
    file: Option<FileId>,
) -> Result<Vec<KExpression<'a>>, KError> {
    let lines = sexlex::read(source).map_err(|e| KError::parse(e.to_string(), Some(e.span)))?;
    let lower = Lower {
        program,
        labels,
        file,
        source,
    };
    lines.iter().map(|line| lower.statement(line)).collect()
}

/// Where a run of items sits, which decides where its parts land and which delimiters are
/// meaningful. A brace carries the pair state machine the run feeds.
enum Context<'c, 'a> {
    Expression,
    List,
    Brace(&'c mut DictFrame<'a>),
}

/// Whether a group that wraps nothing but another group is redundant. In a value expression it
/// is — `((a b))` and `(a b)` name the same call — so it peels. Inside `:(...)` it is not: a
/// paren there is structure the dispatcher reads, and `(Function ((List Number)) -> Bool)` takes
/// one argument whose own type is parenthesized. The mode carries into everything a type
/// expression nests, since the whole payload reaches the dispatcher verbatim.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Wrappers {
    Peel,
    Keep,
}

/// A sigil-led line or a sigil glued to its group, resolved down to the body it captures.
/// `#` keeps the body as data; `$` wraps it in the `EVAL` call the runtime performs.
struct Sigil<'a> {
    kind: char,
    body: ProgramExpression<'a>,
    /// The body's own extent: the parens for `#(...)`, the line past its sigil byte for `#rest`.
    body_span: Span,
    /// The sigil byte alone, which the `EVAL` keyword part takes.
    sigil_span: Span,
    /// Sigil through body: what the part this becomes covers.
    outer: Span,
}

struct Lower<'a, 'l, 's> {
    program: ProgramBrand<'a>,
    labels: &'l LabelInterner,
    file: Option<FileId>,
    source: &'s str,
}

impl<'a, 's> Lower<'a, '_, 's> {
    /// One top-level line. A `#`-led line is a statement that holds a quote; a `$`-led line *is*
    /// the `EVAL` call, since evaluation is what the line asks for.
    fn statement(&self, line: &Item<'s>) -> Result<KExpression<'a>, KError> {
        let Node::Group {
            kind: Kind::Layout,
            items,
        } = &line.node
        else {
            return Err(KError::parse(
                "expected a layout line at the top level",
                Some(line.span),
            ));
        };
        match self.sigil_led_line(items, line.span, Wrappers::Peel)? {
            Some(sigil) if sigil.kind == '$' => Ok(self.eval_call(sigil).node()),
            Some(sigil) => {
                let quoted = self.sigil_part(sigil);
                Ok(self
                    .program
                    .build_expression(&[quoted], Some(line.span), self.file)
                    .node())
            }
            None => Ok(self.lower_body(items, line.span, Wrappers::Peel)?.node()),
        }
    }

    /// A run of items that becomes a node. A body that is exactly one group names the same
    /// expression the group does, so the wrapper comes off the *tree* before anything is built —
    /// `((a b))`, `(a b)` and a one-line paren body all reach the builder as the same run. The
    /// peel stops at a sigil-led line, whose group is the quote's body rather than a wrapper.
    ///
    /// A compound atom reaches the same shape through classification rather than through a group
    /// (`a.b` is one `ATTR` sub-expression), so a run that came out as a single sub-expression is
    /// unwrapped too — a line reading `a.b` is that call, not a statement holding it. The
    /// outermost span is stamped on the survivor so diagnostics point at what the user wrote.
    fn lower_body(
        &self,
        mut items: &[Item<'s>],
        span: Span,
        wrappers: Wrappers,
    ) -> Result<ProgramExpression<'a>, KError> {
        if wrappers == Wrappers::Peel {
            while let [only] = items
                && let Some(inner) = peelable(only)
            {
                items = inner;
            }
        }
        let mut parts = self.lower_run(items.iter(), &mut Context::Expression, wrappers)?;
        admit_bare_type_slots(&mut parts);
        let mut survivor: Option<KExpression<'a>> = None;
        while wrappers == Wrappers::Peel
            && let [
                Spanned {
                    value: ExpressionPart::Expression(node),
                    ..
                },
            ] = survivor.map_or(parts.as_slice(), |expression| expression.parts)
        {
            survivor = Some(**node);
        }
        Ok(match survivor {
            Some(expression) => self.program.build_expression_from_iter(
                expression.parts.iter().copied(),
                Some(span),
                self.file,
            ),
            None => self
                .program
                .build_expression_from_iter(parts, Some(span), self.file),
        })
    }

    /// Walk a run of sibling items in order, emitting the parts they stand for. A sigil consumes
    /// the group after it, so the walk keeps a lookahead; adjacency reads the sibling before.
    fn lower_run<'i, I>(
        &self,
        items: I,
        context: &mut Context<'_, 'a>,
        wrappers: Wrappers,
    ) -> Result<Vec<Spanned<ExpressionPart<'a>>>, KError>
    where
        I: Iterator<Item = &'i Item<'s>>,
        's: 'i,
    {
        let mut items = items.peekable();
        let mut parts = Vec::new();
        while let Some(item) = items.next() {
            match &item.node {
                Node::Atom(text) => {
                    // A sigil is an atom glued to the group after it; the pair is one part.
                    if let Some(sigil) = sigil_kind(text)
                        && let Some(group) = items.peek()
                        && item.glued
                        && let Node::Group { kind, items: inner } = &group.node
                        && sigil_takes(sigil, *kind)
                    {
                        let kind = *kind;
                        let group = items.next().expect("just peeked");
                        let part =
                            self.glued_sigil_part(sigil, item, group, kind, inner, wrappers)?;
                        self.push_part(context, &mut parts, part)?;
                        continue;
                    }
                    self.lower_atom(text, item, context, &mut parts)?;
                }
                Node::Str { body, .. } => {
                    let text = self.program.region().allocator().text(body);
                    let part =
                        Spanned::at(ExpressionPart::Literal(KLiteral::String(text)), item.span);
                    self.push_part(context, &mut parts, part)?;
                }
                // A comma pairs entries inside a brace and is whitespace everywhere else, so
                // annotation runs like `a :Number, b :Str` read the same either way.
                Node::Comma => {
                    if let Context::Brace(dict) = context {
                        dict.accept_comma()?;
                    }
                }
                Node::Group {
                    kind: kind @ (Kind::Paren | Kind::Layout),
                    items: inner,
                } => {
                    let part = self.group_part(item, inner, *kind, wrappers)?;
                    self.push_part(context, &mut parts, part)?;
                }
                Node::Group {
                    kind: kind @ (Kind::Bracket | Kind::Brace),
                    items: inner,
                } => {
                    self.check_adjacency(item, *kind)?;
                    let part = self.collection_part(item, inner, *kind, wrappers)?;
                    self.push_part(context, &mut parts, part)?;
                }
            }
        }
        Ok(parts)
    }

    /// A list, dict or record literal holds values, and the only keyword-shaped things its syntax
    /// has are the `,` / `:` / `=` delimiters the run consumes itself — so a keyword reaching one
    /// is refused here, the single funnel every part passes through. Every other run still admits
    /// keywords, which is what `#(+)` bodies and `OP` declarations rest on.
    fn push_part(
        &self,
        context: &mut Context<'_, 'a>,
        parts: &mut Vec<Spanned<ExpressionPart<'a>>>,
        part: Spanned<ExpressionPart<'a>>,
    ) -> Result<(), KError> {
        if !matches!(context, Context::Expression)
            && let ExpressionPart::Keyword(symbol) = part.value
        {
            return Err(KError::parse(
                format!(
                    "`{}` is a keyword, so it cannot be an element of a list, dict, or record \
                     literal",
                    self.labels.display(symbol.symbol()),
                ),
                part.span,
            ));
        }
        match context {
            Context::Brace(dict) => dict.push(part.value),
            _ => parts.push(part),
        }
        Ok(())
    }

    /// One atom. A bare `#` / `$` that reached here took no group, and neither did one written
    /// with its operand glued on (`#3`). A bare `:`, and the `:` an atom like `a:` ends in, either
    /// pairs a brace entry or names nothing — and a pairing separator closes the key before it, so
    /// the atom's own parts land first.
    fn lower_atom(
        &self,
        text: &str,
        item: &Item<'s>,
        context: &mut Context<'_, 'a>,
        parts: &mut Vec<Spanned<ExpressionPart<'a>>>,
    ) -> Result<(), KError> {
        if let Some(sigil) = sigil_kind(text) {
            if sigil != ':' {
                return Err(self.missing_sigil_group(sigil, item.span.start));
            }
            return match context {
                Context::Brace(dict) => dict.accept_colon(),
                _ => Err(self.colon_error(item.span.end)),
            };
        }
        if let Some(sigil @ ('#' | '$')) = text.chars().next() {
            return Err(self.missing_sigil_group(sigil, item.span.start));
        }
        // Inside a brace `=` is the record-pair separator (`{x = 1}`); everywhere else it stays an
        // ordinary keyword token (`LET x = 1`, kwargs, FN bodies).
        if text == "="
            && let Context::Brace(dict) = context
        {
            return dict.accept_equals();
        }
        let classified = atom::classify(self.program, self.labels, text, item.span)?;
        for part in classified.parts {
            self.push_part(context, parts, part)?;
        }
        if classified.trailing_colon {
            return match context {
                Context::Brace(dict) => dict.accept_colon(),
                _ => Err(self.colon_error(item.span.end)),
            };
        }
        Ok(())
    }

    /// A `(...)` or a body line: a nested expression. A body line that leads with `#` or `$`
    /// quotes itself instead.
    fn group_part(
        &self,
        item: &Item<'s>,
        inner: &[Item<'s>],
        kind: Kind,
        wrappers: Wrappers,
    ) -> Result<Spanned<ExpressionPart<'a>>, KError> {
        if kind == Kind::Layout
            && let Some(sigil) = self.sigil_led_line(inner, item.span, wrappers)?
        {
            return Ok(self.sigil_part(sigil));
        }
        let expr = self.lower_body(inner, item.span, wrappers)?;
        Ok(Spanned::at(
            ExpressionPart::Expression(self.program.alloc_node(expr)),
            item.span,
        ))
    }

    /// A `[...]` list literal, or a `{...}` brace literal — a dict (`:` pairs) or a record
    /// (`=` pairs), whichever its first pairing operator selects.
    fn collection_part(
        &self,
        item: &Item<'s>,
        inner: &[Item<'s>],
        kind: Kind,
        wrappers: Wrappers,
    ) -> Result<Spanned<ExpressionPart<'a>>, KError> {
        let allocator = self.program.region().allocator();
        if kind == Kind::Bracket {
            let parts = self.lower_run(inner.iter(), &mut Context::List, wrappers)?;
            let items = allocator.slice_from_iter(parts.into_iter().map(|part| part.value));
            return Ok(Spanned::at(ExpressionPart::ListLiteral(items), item.span));
        }
        let mut dict = DictFrame::new(self.program);
        self.lower_run(inner.iter(), &mut Context::Brace(&mut dict), wrappers)?;
        let part = match dict.finish(self.labels)? {
            BraceContents::Dict(pairs) => {
                ExpressionPart::DictLiteral(allocator.slice_from_iter(pairs))
            }
            BraceContents::Record(fields) => {
                ExpressionPart::RecordLiteral(allocator.slice_from_iter(fields))
            }
        };
        Ok(Spanned::at(part, item.span))
    }

    /// A sigil glued to its group. `#` and `$` take an expression body; `:` takes a type
    /// expression (`:(List Number)`) or a record type (`:{x :Number}`), both of which the parser
    /// stores verbatim — reading a shape out of the payload is the dispatcher's job.
    fn glued_sigil_part(
        &self,
        sigil_kind: char,
        sigil: &Item<'s>,
        group: &Item<'s>,
        group_kind: Kind,
        inner: &[Item<'s>],
        wrappers: Wrappers,
    ) -> Result<Spanned<ExpressionPart<'a>>, KError> {
        let outer = Span {
            start: sigil.span.start,
            end: group.span.end,
        };
        if sigil_kind != ':' {
            let body = self.lower_body(inner, group.span, wrappers)?;
            return Ok(self.sigil_part(Sigil {
                kind: sigil_kind,
                body,
                body_span: group.span,
                sigil_span: Span {
                    start: sigil.span.start,
                    end: sigil.span.start + 1,
                },
                outer,
            }));
        }
        // A `:{...}` closes like any brace literal, so its closer takes the same rule.
        if group_kind == Kind::Brace {
            self.check_close_adjacency(group, Kind::Brace)?;
        }
        let parts = self.lower_run(inner.iter(), &mut Context::Expression, Wrappers::Keep)?;
        // A `:(...)` holding a lone sub-expression is re-labelled rather than re-wrapped:
        // `:(Point.x)` yields the same one-node part `build_attr` emits for a Type-class tail,
        // and `:(:(…))` is idempotent. Normalization only — the parser still reads no meaning
        // out of the payload's shape.
        if group_kind == Kind::Paren
            && let [only] = parts.as_slice()
            && let ExpressionPart::Expression(node) | ExpressionPart::SigiledTypeExpr(node) =
                only.value
        {
            return Ok(Spanned::at(ExpressionPart::SigiledTypeExpr(node), outer));
        }
        let node = self
            .program
            .alloc_node(
                self.program
                    .build_expression_from_iter(parts, Some(outer), self.file),
            );
        let part = if group_kind == Kind::Paren {
            ExpressionPart::SigiledTypeExpr(node)
        } else {
            // `:{x :Number}` is a first-class part the elaborator folds straight to a record
            // `KType`; the inner node is the bare `(x :Number, …)` field list.
            ExpressionPart::RecordType(node)
        };
        Ok(Spanned::at(part, outer))
    }

    /// A layout line whose first atom starts with `#` or `$` quotes (or evaluates) the whole
    /// line, its child lines included: the sigil byte comes off the first atom and everything
    /// left is the body.
    fn sigil_led_line(
        &self,
        items: &[Item<'s>],
        span: Span,
        wrappers: Wrappers,
    ) -> Result<Option<Sigil<'a>>, KError> {
        let Some(kind) = sigil_lead(items) else {
            return Ok(None);
        };
        let first = &items[0];
        let Node::Atom(text) = &first.node else {
            unreachable!("sigil_lead matched an atom")
        };
        let text: &'s str = text;
        let body_span = Span {
            start: first.span.start + 1,
            end: span.end,
        };
        let rest = &text[1..];
        let body = if rest.is_empty() {
            self.lower_body(&items[1..], body_span, wrappers)?
        } else {
            let rest = Item {
                node: Node::Atom(rest),
                span: Span {
                    start: first.span.start + 1,
                    end: first.span.end,
                },
                glued: first.glued,
            };
            let mut parts = self.lower_run(
                std::iter::once(&rest).chain(items[1..].iter()),
                &mut Context::Expression,
                wrappers,
            )?;
            admit_bare_type_slots(&mut parts);
            self.program
                .build_expression_from_iter(parts, Some(body_span), self.file)
        };
        Ok(Some(Sigil {
            kind,
            body,
            body_span,
            sigil_span: Span {
                start: first.span.start,
                end: first.span.start + 1,
            },
            outer: span,
        }))
    }

    fn sigil_part(&self, sigil: Sigil<'a>) -> Spanned<ExpressionPart<'a>> {
        let outer = sigil.outer;
        let part = match sigil.kind {
            // `#(...)` captures its body as data: no keyword head, no call.
            '#' => ExpressionPart::QuotedExpression(self.program.alloc_node(sigil.body)),
            // `$(...)` keeps the head channel — evaluation is a runtime operation.
            _ => ExpressionPart::Expression(self.program.alloc_node(self.eval_call(sigil))),
        };
        Spanned::at(part, outer)
    }

    fn eval_call(&self, sigil: Sigil<'a>) -> ProgramExpression<'a> {
        let head = KeywordSymbol::declared("EVAL", self.labels).expect("`EVAL` is keyword-class");
        self.program.build_expression(
            &[
                Spanned::at(ExpressionPart::Keyword(head), sigil.sigil_span),
                Spanned::at(
                    ExpressionPart::Expression(self.program.alloc_node(sigil.body)),
                    sigil.body_span,
                ),
            ],
            Some(sigil.outer),
            self.file,
        )
    }

    /// A collection literal can't be glued to a token on either side: `foo[1]` would read as an
    /// index and `[1]foo` as an application, and koan spells neither that way.
    fn check_adjacency(&self, item: &Item<'s>, kind: Kind) -> Result<(), KError> {
        let opener = kind.delimiters().expect("bracket kinds carry delimiters").0;
        if let Some(previous) = self.char_before(item.span.start)
            && !previous.is_whitespace()
            && !matches!(previous, '(' | '[' | '{')
        {
            return Err(KError::parse(
                format!(
                    "'{opener}' must be preceded by whitespace, '(', '[', or '{{' \
                     (got {:?}); collection literals can't be glued to a token",
                    Some(previous),
                ),
                Some(item.span),
            ));
        }
        self.check_close_adjacency(item, kind)
    }

    fn check_close_adjacency(&self, item: &Item<'s>, kind: Kind) -> Result<(), KError> {
        let closer = kind.delimiters().expect("bracket kinds carry delimiters").1;
        let next = self.source[item.span.end as usize..].chars().next();
        if matches!(next, None | Some(')' | ']' | '}'))
            || matches!(next, Some(c) if c.is_whitespace())
        {
            return Ok(());
        }
        Err(KError::parse(
            format!(
                "'{closer}' must be followed by whitespace, ')', ']', or '}}' \
                 (got {next:?}); collection literals can't be glued to a token",
            ),
            Some(item.span),
        ))
    }

    /// A `:` that named no type. Which mistake it was is in what follows it: nothing at all, a
    /// space (the annotation lost its operand), or a token that is no type name.
    fn colon_error(&self, at: u32) -> KError {
        match self.char_at(at) {
            None => KError::parse(
                "trailing ':' at end of input; expected a type name or `(`",
                None,
            ),
            Some(c) if c.is_whitespace() => KError::parse(
                "':' must be glued to its operand at a type position; \
                 write `name :Type` (no space after `:`) or `:(List ...)`",
                None,
            ),
            Some(c) => KError::parse(atom::not_a_type_name(c), None),
        }
    }

    /// `#` and `$` take a `(...)` group and nothing else. The message names what was found in its
    /// place, which is the character right after the sigil byte.
    fn missing_sigil_group(&self, sigil: char, at: u32) -> KError {
        match self.source[at as usize..].chars().nth(1) {
            Some(c) => KError::parse(format!("expected '(' after '{sigil}', found '{c}'"), None),
            None => KError::parse(
                format!("trailing '{sigil}' sigil at end of input; expected '('"),
                None,
            ),
        }
    }

    fn char_before(&self, at: u32) -> Option<char> {
        self.source[..at as usize].chars().next_back()
    }

    fn char_at(&self, at: u32) -> Option<char> {
        self.source[at as usize..].chars().next()
    }
}

/// The group kinds a sigil may take: `#` and `$` capture an expression, `:` a type expression or
/// a record type.
fn sigil_takes(sigil: char, kind: Kind) -> bool {
    match sigil {
        ':' => matches!(kind, Kind::Paren | Kind::Brace),
        _ => kind == Kind::Paren,
    }
}

/// An atom that is exactly one sigil character.
fn sigil_kind(text: &str) -> Option<char> {
    match text {
        "#" => Some('#'),
        "$" => Some('$'),
        ":" => Some(':'),
        _ => None,
    }
}

/// The sigil a layout line leads with, if any.
fn sigil_lead(items: &[Item<'_>]) -> Option<char> {
    let Node::Atom(text) = &items.first()?.node else {
        return None;
    };
    match text.chars().next() {
        Some(c @ ('#' | '$')) => Some(c),
        _ => None,
    }
}

/// The items a single-group body collapses to, when the group is a wrapper rather than a
/// quote's own body. A bracket or brace is a literal, never a wrapper.
fn peelable<'r, 's>(item: &'r Item<'s>) -> Option<&'r [Item<'s>]> {
    match &item.node {
        Node::Group {
            kind: Kind::Paren,
            items,
        } => Some(items),
        Node::Group {
            kind: Kind::Layout,
            items,
        } if sigil_lead(items).is_none() => Some(items),
        _ => None,
    }
}

/// One line's parts run, built straight from [`Lower::lower_run`] — the shape the tests assert
/// against, where a redundant wrapper is still visible. It skips what [`Lower::lower_body`] adds
/// around that run, the peel and `admit_bare_type_slots`, so an expectation here names the parts
/// a paren or sigil produced. Rejects input that is not exactly one line.
#[cfg(test)]
pub(super) fn lower_run_for_tests<'a>(
    program: ProgramBrand<'a>,
    labels: &LabelInterner,
    source: &str,
) -> Result<KExpression<'a>, KError> {
    let lines = sexlex::read(source).map_err(|e| KError::parse(e.to_string(), Some(e.span)))?;
    let line = match lines.as_slice() {
        [] => return Ok(program.build_expression(&[], None, None).node()),
        [line] => line,
        _ => return Err(KError::parse("this helper reads exactly one line", None)),
    };
    let Node::Group {
        kind: Kind::Layout,
        items,
    } = &line.node
    else {
        unreachable!("read yields one layout group per top-level line")
    };
    let lower = Lower {
        program,
        labels,
        file: None,
        source,
    };
    let parts = lower.lower_run(items.iter(), &mut Context::Expression, Wrappers::Peel)?;
    Ok(program
        .build_expression_from_iter(parts, Some(line.span), None)
        .node())
}
