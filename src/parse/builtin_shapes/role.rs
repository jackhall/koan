//! What one part of a builtin shape is to name resolution.
//!
//! A part's role decides whether a name in it is a mention at all, and how the mention's class
//! moves on the way down — see
//! [scope/README.md § Visibility](../../scope/README.md#visibility). Every
//! [`BUILTIN_SHAPES`](super::BUILTIN_SHAPES) element carries its own role, so a shape added to the
//! table gives its parts roles where it is spelled.

/// What one part of a builtin shape is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// A fixed token of the shape.
    Keyword,
    /// The binder's own name, or a `VAL`'s member name: declared, not read.
    Name,
    /// A binding's right-hand side: classification starts here.
    Rhs,
    /// A slot whose value the shape needs where it runs.
    Argument,
    /// A type expression, read where the shape runs.
    TypeExpression,
    /// A `TRY` or `CATCH` operand: code that runs where it is written, once, as a part of the
    /// enclosing shape.
    InPlace,
    /// A `FN`'s `:{…}` schema: names declared by the body, types read where the shape runs.
    Signature,
    /// An `EXPR` head: a quote whose names the body declares as parameters, with the types read
    /// where the shape runs.
    Head,
    /// A `FOR ALL` group: a list of name quotes, or a dict of name quotes to bound quotes — type
    /// parameters the body declares.
    Quantifiers,
    /// A body that is its own body shape.
    Body(BodyKind),
    /// A dict of guard quotes to arm quotes, each arm a block shape.
    Branches(Heads),
    /// A type declaration's definition, after the `=`: a constructor context whose own symbols and
    /// declarations are not mentions.
    Definition(DefinitionKind),
    /// A quoted symbol: data, never read.
    Data,
    /// A field label: a bare name is the label itself; any other part is evaluated.
    Field,
    /// A part of a shape the builder does not resolve.
    Unsupported,
}

/// How the shape builder reads one part of a builtin shape. A part that runs later, conditionally
/// or never is a quote; one that declares, or runs where it is written, once, is bare; one that
/// names things as data is a container of quotes; every other part is evaluated.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reading {
    /// A written quote.
    Quote,
    /// Bare syntax that declares, or runs where it is written, once.
    Bare,
    /// A bare list or dict literal whose every element is a written quote.
    Container,
    /// A bare name is read as written; any other part is evaluated.
    Label,
    /// Evaluated where the shape runs.
    Evaluated,
}

impl Role {
    /// How the builder reads a part under this role — the one authority the builder, the operator
    /// claims scan and the rewrite each ask, so no position list sits beside the table.
    pub const fn reading(self) -> Reading {
        match self {
            Role::Body(BodyKind::Lambda | BodyKind::Operator | BodyKind::UnaryOperator)
            | Role::Head
            | Role::Data => Reading::Quote,
            Role::Body(BodyKind::Module | BodyKind::Surfaced)
            | Role::Definition(DefinitionKind::Plain)
            | Role::Name
            | Role::TypeExpression
            | Role::InPlace
            | Role::Keyword => Reading::Bare,
            Role::Branches(_)
            | Role::Quantifiers
            | Role::Definition(DefinitionKind::Union | DefinitionKind::Members) => {
                Reading::Container
            }
            Role::Field => Reading::Label,
            Role::Argument | Role::Rhs | Role::Signature | Role::Unsupported => Reading::Evaluated,
        }
    }
}

/// What a body slot opens, and the parameters it declares.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BodyKind {
    /// A `FN` or `EXPR` body: the signature's parameters.
    Lambda,
    /// A binary `OP` body: `left` and `right`.
    Operator,
    /// A `UNARY OP` body: `operands`.
    UnaryOperator,
    /// A `MODULE` or `GROUP` body: no parameters, an eager context.
    Module,
    /// A `USING` body: a block whose parameters are the names its operand surfaces.
    Surfaced,
}

/// What an arm's head is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Heads {
    /// A type name read where the match runs (`MATCH … WITH`).
    Types,
    /// A member or error-kind label (`MATCH … OVER`, `TRY`).
    Labels,
}

/// How a definition lays out its symbols.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DefinitionKind {
    /// `UNION`: a dict of tag quotes to payload-type quotes.
    Union,
    /// A list of member declarations: a `SIG` body, or the heads a bodyless `GROUP` declares.
    Members,
    /// `NEWTYPE`'s representation: a type expression whose own symbols are not mentions.
    Plain,
}
