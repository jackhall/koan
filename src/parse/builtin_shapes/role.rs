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
    /// The binder's own name: declared, not read.
    Name,
    /// A binding's right-hand side: classification starts here.
    Rhs,
    /// A slot whose value the shape needs where it runs.
    Argument,
    /// A type expression, read where the shape runs.
    TypeExpression,
    /// A callable's parameter list: names declared by the body, types read where the shape runs.
    Signature,
    /// A `FOR ALL` group: type parameters the body declares.
    Quantifiers,
    /// A body that is its own body shape.
    Body(BodyKind),
    /// `<head> -> <body>` arms, each body a block shape.
    Branches(Heads),
    /// A type declaration's definition, after the `=`: a constructor context whose own labels and
    /// declarations are not mentions.
    Definition(DefinitionKind),
    /// A quoted symbol or quoted code: data, never read.
    Data,
    /// A field or member label: never read.
    Label,
    /// A part of a shape the builder does not resolve.
    Unsupported,
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
}

/// What an arm's head is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Heads {
    /// A type name read where the match runs (`MATCH … WITH`).
    Types,
    /// A member or error-kind label (`MATCH … OVER`, `TRY`).
    Labels,
}

/// How a definition lays out its labels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DefinitionKind {
    /// `UNION`: tag, type, tag, type.
    Union,
    /// Everything else: identifiers are labels, `TYPE` declarations are the definition's own.
    Plain,
}
