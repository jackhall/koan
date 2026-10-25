//! A **composition's facts**: what the load does for a program, done for code composed at run
//! time. Its code shape is built alone ([`Shapes::of_code`](super::Shapes::of_code)), the
//! language's walk runs rooted at it with nothing bound, and the folds and closed trees that walk
//! computes are laid in the composition's own storage, as the load's are in program storage. Beside
//! them it keeps a [`Term`] of each type its shapes or records name that the root and the agent
//! tier do not hold, which a frame under it mints into its own chain, so the composition rests
//! nothing in the agent tier; the wording of the refusal its code reached, if any, which the
//! `EVAL` running it reports; and the path through the code's syntax to each quote nested in it,
//! so a quote born of composed code reads its syntax inside the root code value's, which outlives
//! the composition.
//!
//! The arena owns one per syntax digest. A step under it reads it through [`Facts`](super::Facts),
//! as the base of its chain in place of the load.

use self_cell::self_cell;

use crate::memory::{
    BumpAllocator, BumpVec, Lays, ProgramBrand, ProgramStorage, collect, program_storage,
};
use crate::parse::{ExpressionPart, KExpression, ProgramNode, copied};
use crate::type_lattice::Term;

use super::records::{ComposedRoot, LoadFacts};
use super::shape::{Located, Segment, ShapeKind};
use super::shapes::Shapes;

self_cell!(
    /// Composed code's syntax, copied whole into program storage of its own: what a cell that
    /// bounces a miss hands the drain, and what the composition built from it keeps, its shapes
    /// and records laid in the same storage over the copy.
    pub struct ComposedSyntax {
        owner: ProgramStorage,
        #[covariant]
        dependent: ProgramNode,
    }
);

impl ComposedSyntax {
    /// `root`'s syntax copied into fresh storage.
    pub fn of(root: ComposedRoot<'_>) -> Self {
        ComposedSyntax::new(program_storage(), |storage| {
            copied(storage.brand(), root.syntax.reference())
        })
    }

    /// The copy.
    pub fn syntax(&self) -> ProgramNode<'_> {
        *self.borrow_dependent()
    }

    /// Its storage's brand, which what is built over the copy lays through.
    pub fn brand(&self) -> ProgramBrand<'_> {
        self.borrow_owner().brand()
    }

    /// The bytes laid in its storage so far.
    pub fn allocated_bytes(&self) -> usize {
        self.borrow_owner().allocated_bytes()
    }
}

/// One composition's facts: see the module documentation.
#[derive(Clone, Copy)]
pub struct Composition<'a> {
    /// Its own copy of the root syntax, which its shapes are built over.
    syntax: ProgramNode<'a>,
    facts: &'a LoadFacts<'a>,
    terms: &'a [Term<'a>],
    refusal: Option<&'a str>,
    /// Each code shape's path through the root's syntax, by segment: empty for a body that is no
    /// nested quote's.
    quotes: &'a [&'a [u32]],
}

impl<'a> Composition<'a> {
    pub fn new(
        syntax: ProgramNode<'a>,
        facts: &'a LoadFacts<'a>,
        terms: &'a [Term<'a>],
        refusal: Option<&'a str>,
        quotes: &'a [&'a [u32]],
    ) -> Self {
        Composition {
            syntax,
            facts,
            terms,
            refusal,
            quotes,
        }
    }

    /// Its own copy of the root syntax.
    pub fn syntax(&self) -> ProgramNode<'a> {
        self.syntax
    }

    /// Its walk's facts over its own shapes.
    pub fn facts(&self) -> &'a LoadFacts<'a> {
        self.facts
    }

    /// A term of each type its facts name that a chain over the root and the agent tier does not
    /// resolve.
    pub fn terms(&self) -> &'a [Term<'a>] {
        self.terms
    }

    /// The wording of the refusal its code reached, as the load words one.
    pub fn refusal(&self) -> Option<&'a str> {
        self.refusal
    }

    /// The syntax of the quote whose code shape is `segment`, inside `root`, a root syntax equal
    /// to this composition's: the paths are structural, so a value's own copy finds its quotes.
    pub fn quote_in<'g>(&self, root: ProgramNode<'g>, segment: Segment) -> ProgramNode<'g> {
        let path = self.quotes[segment.index() as usize];
        let (first, down) = path
            .split_first()
            .expect("a nested quote lies below its root");
        let mut part = &root.reference().parts[*first as usize].value;
        for at in down {
            part = child(part, *at as usize).expect("a path names a child its part has");
        }
        match part {
            ExpressionPart::QuotedExpression(node) => *node,
            _ => unreachable!("a quote's path ends at its quoted expression"),
        }
    }
}

/// Child `at` of `part`: a nested expression's parts, a list's items, a dict's keys and values in
/// turn, a record's values. `None` past the last, and for a part that nests nothing.
fn child<'g>(part: &ExpressionPart<'g>, at: usize) -> Option<&'g ExpressionPart<'g>> {
    match part {
        ExpressionPart::Expression(node)
        | ExpressionPart::SigiledTypeExpr(node)
        | ExpressionPart::RecordType(node)
        | ExpressionPart::QuotedExpression(node)
        | ExpressionPart::MarkedUse(_, node) => {
            node.reference().parts.get(at).map(|part| &part.value)
        }
        ExpressionPart::ListLiteral(items) => items.get(at),
        ExpressionPart::DictLiteral(pairs) => pairs
            .get(at / 2)
            .map(|(key, value)| if at.is_multiple_of(2) { key } else { value }),
        ExpressionPart::RecordLiteral(fields) => fields.get(at).map(|(_, value)| value),
        _ => None,
    }
}

/// Each code shape's path through `syntax`, the root its `shapes` were built over, by segment,
/// laid through `lay`: the index of each part on the way down to the quote it shapes, by
/// [`child`]'s numbering. Empty for the root and for every body that is no quote's code.
pub fn quote_paths<'a>(
    lay: impl Lays<'a>,
    scratch: BumpAllocator<'_>,
    shapes: &Shapes<'_>,
    syntax: &KExpression<'_>,
) -> &'a [&'a [u32]] {
    // Every quote in the syntax, by the address of its node's parts, beside its path.
    let mut quotes = BumpVec::new_in(scratch);
    let mut path = BumpVec::new_in(scratch);
    fn walk<'x>(
        part: &ExpressionPart<'_>,
        path: &mut BumpVec<'x, u32>,
        quotes: &mut BumpVec<'_, (usize, &'x [u32])>,
        scratch: BumpAllocator<'x>,
    ) {
        if let ExpressionPart::QuotedExpression(node) = part {
            let at = node.reference().parts.as_ptr().addr();
            quotes.push((at, &*scratch.alloc_slice_copy(path)));
        }
        let mut at = 0;
        while let Some(inner) = child(part, at) {
            path.push(at as u32);
            walk(inner, path, quotes, scratch);
            path.pop();
            at += 1;
        }
    }
    for (at, part) in syntax.parts.iter().enumerate() {
        path.push(at as u32);
        walk(&part.value, &mut path, &mut quotes, scratch);
        path.pop();
    }
    let bodies = shapes.bodies();
    let mut paths: BumpVec<'_, &'a [u32]> = BumpVec::new_in(scratch);
    paths.resize(bodies.len(), &[]);
    for shape in bodies {
        for (site, nested) in shape.nested_shapes() {
            if nested.kind() != ShapeKind::Code {
                continue;
            }
            let Located::Part(ExpressionPart::QuotedExpression(node)) = shape.located(*site) else {
                continue;
            };
            let at = node.reference().parts.as_ptr().addr();
            let (_, found) = (quotes.iter())
                .find(|(quote, _)| *quote == at)
                .expect("each quote's code shape is a quote of the syntax");
            paths[nested.segment().index() as usize] = collect(lay, found.iter().copied());
        }
    }
    collect(lay, paths.iter().copied())
}
