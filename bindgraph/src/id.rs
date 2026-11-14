//! The graph's handles and the bind site a name or question sits at.
//!
//! Every id is a dense `u32` index into one table: [`BodyId`], [`NameId`], [`QuestionId`],
//! [`EdgeId`] and [`AnswersId`] into the [`Text`](crate::Text) that made them and the
//! [`Graph`](crate::Graph) laid from it, [`NodeId`] into the graph's nodes, and [`ComponentId`]
//! into the graph's text components. Only this crate makes one, so an id always names something
//! its table holds.

macro_rules! id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(u32);

        impl $name {
            /// The id at `index` in its table.
            pub(crate) fn new(index: usize) -> Self {
                Self(u32::try_from(index).expect(concat!(stringify!($name), " overflows u32")))
            }

            /// This id's index in its table.
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

id! {
    /// A body of the text: the root, or one nested in another.
    BodyId
}
id! {
    /// A name: bound at one bind site, or given its content by an answer.
    NameId
}
id! {
    /// A `SiteQuestion`: what the text asks at one site.
    QuestionId
}
id! {
    /// A `BindingNode`: a body's entry, or a statement whose name's content the run gives.
    NodeId
}
id! {
    /// A `PassesName` edge: one call's mapping into a callee's entry.
    EdgeId
}
id! {
    /// An `AnswersName` edge: one way an answer gives an answered name its content.
    AnswersId
}
id! {
    /// A text component: a strongly connected component of the text's arcs of need that holds a
    /// cycle, where a knot can form.
    ComponentId
}

/// A place in the text where names receive their content.
///
/// Within one body, the entry binds first, then each statement in index order. Across bodies the
/// order is [`Graph::site_le`](crate::Graph::site_le)'s, which the text's captures decide.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Site {
    /// A body's entry: where its parameters and `FOR ALL` names are bound.
    Entry(BodyId),
    /// The statement at an index within its body.
    Statement(BodyId, u32),
}

impl Site {
    /// The body this site is in.
    pub fn body(self) -> BodyId {
        match self {
            Site::Entry(body) | Site::Statement(body, _) => body,
        }
    }

    /// The site's position within its body: the entry is 0, statement `i` is `i + 1`.
    pub(crate) fn rank(self) -> u64 {
        match self {
            Site::Entry(_) => 0,
            Site::Statement(_, index) => u64::from(index) + 1,
        }
    }
}
