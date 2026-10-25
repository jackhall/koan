//! A **composition's facts**: what the load does for a program, done for code composed at run
//! time. Its code shape is built alone ([`Shapes::of_code`](super::Shapes::of_code)), the
//! language's walk runs rooted at it with nothing bound, and the folds and closed trees that walk
//! computes are laid in the composition's own storage, as the load's are in program storage. Beside
//! them it keeps a [`Term`] of each type its shapes or records name that the root and the agent
//! tier do not hold, which a frame under it mints into its own chain, so the composition rests
//! nothing in the agent tier; and the wording of the refusal its code reached, if any, which the
//! `EVAL` running it reports.
//!
//! The arena owns one per syntax digest. A step under it reads it through [`Facts`](super::Facts),
//! as the base of its chain in place of the load.

use crate::type_lattice::Term;

use super::records::LoadFacts;

/// One composition's facts: see the module documentation.
#[derive(Clone, Copy)]
pub struct Composition<'a> {
    facts: &'a LoadFacts<'a>,
    terms: &'a [Term<'a>],
    refusal: Option<&'a str>,
}

impl<'a> Composition<'a> {
    pub fn new(facts: &'a LoadFacts<'a>, terms: &'a [Term<'a>], refusal: Option<&'a str>) -> Self {
        Composition {
            facts,
            terms,
            refusal,
        }
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
}
