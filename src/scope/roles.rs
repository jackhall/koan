//! The **role table**: what each part of a builtin form is to name resolution.
//!
//! One exhaustive `match` over [`BuiltinShapeId`], so a form added to the table is a compile error here
//! until its parts are given roles. A part's role decides whether a name in it is a mention at all,
//! and how the mention's class moves on the way down — see
//! [README.md § Visibility](README.md#visibility).

use crate::parse::builtin_shapes::BuiltinShapeId;

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
    /// A type declaration's definition, after the `=`: a constructor context whose own labels
    /// and declarations are not mentions.
    Definition(DefinitionKind),
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

/// How a definition lays out its labels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DefinitionKind {
    /// `UNION`: tag, type, tag, type.
    Union,
    /// Everything else: identifiers are labels, `TYPE` declarations are the definition's own.
    Plain,
}

use BodyKind::{Lambda, Module, Operator, UnaryOperator};
use Role::{
    Argument, Body, Branches, Data, Definition, Keyword as Kw, Label, Name, Quantifiers, Rhs,
    Signature, TypeExpression as Te,
};

/// The role of every part of a node matching `form`, element for element with the form's key.
pub(crate) fn roles(form: BuiltinShapeId) -> &'static [Role] {
    match form {
        BuiltinShapeId::LetValue => &[Kw, Name, Kw, Rhs],
        BuiltinShapeId::TypeDeclaration => &[Kw, Name],
        BuiltinShapeId::Module => &[Kw, Name, Kw, Body(Module)],
        BuiltinShapeId::GroupFoldLeft | BuiltinShapeId::GroupFoldRight => {
            &[Kw, Name, Kw, Kw, Kw, Body(Module)]
        }
        BuiltinShapeId::GroupPairwiseFoldLeft | BuiltinShapeId::GroupPairwiseFoldRight => {
            &[Kw, Name, Kw, Kw, Argument, Kw, Kw, Body(Module)]
        }
        BuiltinShapeId::Sig | BuiltinShapeId::NewTypeDefinition => {
            &[Kw, Name, Kw, Definition(DefinitionKind::Plain)]
        }
        BuiltinShapeId::Union => &[Kw, Name, Kw, Definition(DefinitionKind::Union)],
        BuiltinShapeId::NewTypeDeclaration => &[Kw, Name],
        BuiltinShapeId::Val => &[Kw, Label, Te],
        BuiltinShapeId::Lambda | BuiltinShapeId::ExpressionDefinition => {
            &[Kw, Signature, Kw, Te, Kw, Body(Lambda)]
        }
        BuiltinShapeId::LambdaType | BuiltinShapeId::ExpressionHead => &[Kw, Signature, Kw, Te],
        BuiltinShapeId::QuantifiedExpressionDefinition => {
            &[Kw, Kw, Kw, Quantifiers, Signature, Kw, Te, Kw, Body(Lambda)]
        }
        BuiltinShapeId::QuantifiedExpressionHead => &[Kw, Kw, Kw, Quantifiers, Signature, Kw, Te],
        BuiltinShapeId::CombinedExpression => {
            &[Kw, Name, Kw, Kw, Kw, Signature, Kw, Te, Kw, Body(Lambda)]
        }
        BuiltinShapeId::CombinedQuantifiedExpression => &[
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
        BuiltinShapeId::OperatorDefinition => &[Kw, Data, Kw, Te, Kw, Body(Operator)],
        BuiltinShapeId::OperatorDefinitionReturning => {
            &[Kw, Data, Kw, Te, Kw, Te, Kw, Body(Operator)]
        }
        BuiltinShapeId::UnaryOperatorDefinitionReturning => {
            &[Kw, Kw, Data, Kw, Te, Kw, Te, Kw, Body(UnaryOperator)]
        }
        BuiltinShapeId::OperatorHead => &[Kw, Data, Kw, Te],
        BuiltinShapeId::OperatorHeadReturning => &[Kw, Data, Kw, Te, Kw, Te],
        BuiltinShapeId::UnaryOperatorHeadReturning => &[Kw, Kw, Data, Kw, Te, Kw, Te],
        BuiltinShapeId::CombinedOperator => &[Kw, Name, Kw, Kw, Data, Kw, Te, Kw, Body(Operator)],
        BuiltinShapeId::CombinedOperatorReturning => {
            &[Kw, Name, Kw, Kw, Data, Kw, Te, Kw, Te, Kw, Body(Operator)]
        }
        BuiltinShapeId::CombinedUnaryOperatorReturning => &[
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
        BuiltinShapeId::GroupHeadFoldLeft | BuiltinShapeId::GroupHeadFoldRight => {
            &[Kw, Kw, Kw, Kw, Definition(DefinitionKind::Plain)]
        }
        BuiltinShapeId::GroupHeadPairwiseFoldLeft | BuiltinShapeId::GroupHeadPairwiseFoldRight => {
            &[
                Kw,
                Kw,
                Kw,
                Argument,
                Kw,
                Kw,
                Definition(DefinitionKind::Plain),
            ]
        }
        BuiltinShapeId::Match => &[Kw, Argument, Kw, Te, Kw, Branches(Heads::Types)],
        BuiltinShapeId::MatchOver => &[Kw, Argument, Kw, Te, Kw, Te, Kw, Branches(Heads::Labels)],
        BuiltinShapeId::Try => &[Kw, Argument, Kw, Te, Kw, Branches(Heads::Labels)],
        BuiltinShapeId::Catch | BuiltinShapeId::Eval => &[Kw, Argument],
        BuiltinShapeId::Attribute => &[Kw, Argument, Label],
        BuiltinShapeId::Projection => &[Label, Kw, Argument],
        BuiltinShapeId::CombinedLambda
        | BuiltinShapeId::UnaryOperatorDefinition
        | BuiltinShapeId::UnaryOperatorHead
        | BuiltinShapeId::CombinedUnaryOperator
        | BuiltinShapeId::UsingScope
        | BuiltinShapeId::CloseOver
        | BuiltinShapeId::Close => &[Role::Unsupported],
    }
}
