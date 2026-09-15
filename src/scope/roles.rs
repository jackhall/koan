//! The **role table**: what each part of a builtin form is to name resolution.
//!
//! One exhaustive `match` over [`FormId`], so a form added to the table is a compile error here
//! until its parts are given roles. A part's role decides whether a name in it is a mention at all,
//! and how the mention's class moves on the way down — see
//! [README.md § Visibility](README.md#visibility).

use crate::parse::forms::FormId;

/// What one part of a form is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Role {
    /// A fixed token of the form.
    Keyword,
    /// The binder's own name: declared, not read.
    Name,
    /// A binding's right-hand side: classification starts here.
    Rhs,
    /// A slot whose value the form needs where it runs.
    Argument,
    /// A type expression, read where the form runs.
    TypeExpression,
    /// A callable's parameter list: names declared by the body, types read where the form runs.
    Signature,
    /// A `FOR ALL` group: type parameters the body declares.
    Quantifiers,
    /// A body that is its own shape.
    Body(BodyKind),
    /// `<head> -> <body>` arms, each body a block shape.
    Branches(Heads),
    /// A type's schema: a constructor context whose own labels and declarations are not mentions.
    Schema(SchemaKind),
    /// A quoted symbol or quoted code: data, never read.
    Data,
    /// A field or member label: never read.
    Label,
    /// A part of a form the builder does not resolve.
    Unsupported,
}

/// What a body slot opens, and the parameters it declares.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BodyKind {
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
pub(crate) enum Heads {
    /// A type name read where the match runs (`MATCH … WITH`).
    Types,
    /// A member or error-kind label (`MATCH … OVER`, `TRY`).
    Labels,
}

/// How a schema lays out its labels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SchemaKind {
    /// `UNION`: tag, type, tag, type.
    Union,
    /// Everything else: identifiers are labels, `TYPE` declarations are the schema's own.
    Plain,
}

use BodyKind::{Lambda, Module, Operator, UnaryOperator};
use Role::{
    Argument, Body, Branches, Data, Keyword as Kw, Label, Name, Quantifiers, Rhs, Schema,
    Signature, TypeExpression as Te,
};

/// The role of every part of a node matching `form`, element for element with the form's key.
pub(crate) fn roles(form: FormId) -> &'static [Role] {
    match form {
        FormId::LetValue => &[Kw, Name, Kw, Rhs],
        FormId::TypeDeclaration => &[Kw, Name],
        FormId::Module => &[Kw, Name, Kw, Body(Module)],
        FormId::GroupFoldLeft | FormId::GroupFoldRight => &[Kw, Name, Kw, Kw, Kw, Body(Module)],
        FormId::GroupPairwiseFoldLeft | FormId::GroupPairwiseFoldRight => {
            &[Kw, Name, Kw, Kw, Argument, Kw, Kw, Body(Module)]
        }
        FormId::Sig | FormId::NewTypeDefinition => &[Kw, Name, Kw, Schema(SchemaKind::Plain)],
        FormId::Union => &[Kw, Name, Kw, Schema(SchemaKind::Union)],
        FormId::NewTypeDeclaration => &[Kw, Name],
        FormId::Val => &[Kw, Label, Te],
        FormId::Lambda | FormId::ExpressionDefinition => &[Kw, Signature, Kw, Te, Kw, Body(Lambda)],
        FormId::LambdaType | FormId::ExpressionHead => &[Kw, Signature, Kw, Te],
        FormId::QuantifiedExpressionDefinition => {
            &[Kw, Kw, Kw, Quantifiers, Signature, Kw, Te, Kw, Body(Lambda)]
        }
        FormId::QuantifiedExpressionHead => &[Kw, Kw, Kw, Quantifiers, Signature, Kw, Te],
        FormId::CombinedExpression => &[Kw, Name, Kw, Kw, Kw, Signature, Kw, Te, Kw, Body(Lambda)],
        FormId::CombinedQuantifiedExpression => &[
            Kw,
            Name,
            Kw,
            Kw,
            Kw,
            Kw,
            Kw,
            Quantifiers,
            Signature,
            Kw,
            Te,
            Kw,
            Body(Lambda),
        ],
        FormId::OperatorDefinition => &[Kw, Data, Kw, Te, Kw, Body(Operator)],
        FormId::OperatorDefinitionReturning => &[Kw, Data, Kw, Te, Kw, Te, Kw, Body(Operator)],
        FormId::UnaryOperatorDefinitionReturning => {
            &[Kw, Kw, Data, Kw, Te, Kw, Te, Kw, Body(UnaryOperator)]
        }
        FormId::OperatorHead => &[Kw, Data, Kw, Te],
        FormId::OperatorHeadReturning => &[Kw, Data, Kw, Te, Kw, Te],
        FormId::UnaryOperatorHeadReturning => &[Kw, Kw, Data, Kw, Te, Kw, Te],
        FormId::CombinedOperator => &[Kw, Name, Kw, Kw, Data, Kw, Te, Kw, Body(Operator)],
        FormId::CombinedOperatorReturning => {
            &[Kw, Name, Kw, Kw, Data, Kw, Te, Kw, Te, Kw, Body(Operator)]
        }
        FormId::CombinedUnaryOperatorReturning => &[
            Kw,
            Name,
            Kw,
            Kw,
            Kw,
            Data,
            Kw,
            Te,
            Kw,
            Te,
            Kw,
            Body(UnaryOperator),
        ],
        FormId::GroupHeadFoldLeft | FormId::GroupHeadFoldRight => {
            &[Kw, Kw, Kw, Kw, Schema(SchemaKind::Plain)]
        }
        FormId::GroupHeadPairwiseFoldLeft | FormId::GroupHeadPairwiseFoldRight => {
            &[Kw, Kw, Kw, Argument, Kw, Kw, Schema(SchemaKind::Plain)]
        }
        FormId::Match => &[Kw, Argument, Kw, Te, Kw, Branches(Heads::Types)],
        FormId::MatchOver => &[Kw, Argument, Kw, Te, Kw, Te, Kw, Branches(Heads::Labels)],
        FormId::Try => &[Kw, Argument, Kw, Te, Kw, Branches(Heads::Labels)],
        FormId::Catch | FormId::Eval => &[Kw, Argument],
        FormId::Attribute => &[Kw, Argument, Label],
        FormId::Projection => &[Label, Kw, Argument],
        FormId::CombinedLambda
        | FormId::UnaryOperatorDefinition
        | FormId::UnaryOperatorHead
        | FormId::CombinedUnaryOperator
        | FormId::UsingScope
        | FormId::CloseOver
        | FormId::Close => &[Role::Unsupported],
    }
}
