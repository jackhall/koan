//! The **refusal copier**: a [`ShapeError`] laid again through another lay, so it outlives the
//! memory it was read from — a composition's walk's scratch, or the step that worded a frame's
//! refusal. Every slice and tree it reaches is laid again, so nothing it holds points where it was
//! read from.
//!
//! Each record is destructured whole and each match lists every variant, as the
//! [type visitor](super::record_types)'s are, so a field a record gains does not compile until it
//! is relaid. Types are handles and copy as they are; what resolves them is the caller's to adopt.

use crate::memory::{Lays, collect, resident};

use super::shape::{Repeated, Scalar, ShapeError};

/// `error` laid again through `lay`, every slice it reaches copied: what a composition keeps of
/// the refusal its walk reached, and a frame of the refusal its instance answered. What resolves
/// its types is the caller's to adopt.
pub(crate) fn relaid_error<'to>(
    lay: impl Lays<'to>,
    error: &ShapeError<'_>,
) -> &'to ShapeError<'to> {
    resident(lay, Relay { lay }.error(error))
}

/// A copier laying what it reads through `lay`.
struct Relay<L> {
    lay: L,
}

impl<'to, L: Lays<'to>> Relay<L> {
    /// A slice of plain data laid again.
    fn copied<T: Copy>(&self, values: &[T]) -> &'to [T] {
        collect(self.lay, values.iter().copied())
    }

    /// `error`, laid again.
    fn error(&self, error: &ShapeError<'_>) -> ShapeError<'to> {
        match *error {
            ShapeError::Rebind {
                name,
                first,
                second,
            } => ShapeError::Rebind {
                name,
                first,
                second,
            },
            ShapeError::ShadowsBuiltin { name, at } => ShapeError::ShadowsBuiltin { name, at },
            ShapeError::Unbound { name, site, at } => ShapeError::Unbound { name, site, at },
            ShapeError::QuantifiedValue { at } => ShapeError::QuantifiedValue { at },
            ShapeError::Unfixed {
                variables,
                wanted,
                at,
            } => ShapeError::Unfixed {
                variables: self.copied(variables),
                wanted,
                at,
            },
            ShapeError::NoInstance {
                scheme,
                rebounds,
                wanted,
                at,
            } => ShapeError::NoInstance {
                scheme,
                rebounds: rebounds.laid(self.lay),
                wanted,
                at,
            },
            ShapeError::MemberUninstanced {
                name,
                carried,
                wanted,
                at,
            } => ShapeError::MemberUninstanced {
                name,
                carried,
                wanted,
                at,
            },
            ShapeError::AmbiguousInstance { key, at } => ShapeError::AmbiguousInstance {
                key: self.copied(key),
                at,
            },
            ShapeError::NoInstanceAtCandidates { key, at } => ShapeError::NoInstanceAtCandidates {
                key: self.copied(key),
                at,
            },
            ShapeError::QuantifiedRead { name, site, at } => {
                ShapeError::QuantifiedRead { name, site, at }
            }
            ShapeError::EagerCycle {
                members,
                definitions,
                at,
            } => ShapeError::EagerCycle {
                members: self.copied(members),
                definitions: self
                    .lay
                    .fill(definitions.len(), |index| self.copied(definitions[index])),
                at,
            },
            ShapeError::MarkOutsideQuote { at } => ShapeError::MarkOutsideQuote { at },
            ShapeError::Unsupported { form, at } => ShapeError::Unsupported { form, at },
            ShapeError::Malformed { form, at } => ShapeError::Malformed { form, at },
            ShapeError::Unsurfaced { at, site } => ShapeError::Unsurfaced { at, site },
            ShapeError::Unchained { symbol, at } => ShapeError::Unchained { symbol, at },
            ShapeError::MixedGroups { first, second, at } => {
                ShapeError::MixedGroups { first, second, at }
            }
            ShapeError::RedeclaresGroup { symbol, at } => {
                ShapeError::RedeclaresGroup { symbol, at }
            }
            ShapeError::ResultOutsidePairwise { symbol, at } => {
                ShapeError::ResultOutsidePairwise { symbol, at }
            }
            ShapeError::SpellsForm { symbol, at } => ShapeError::SpellsForm { symbol, at },
            ShapeError::Derived { symbol, at } => ShapeError::Derived { symbol, at },
            ShapeError::Unquoted { form, part, at } => ShapeError::Unquoted { form, part, at },
            ShapeError::Inadmissible {
                form,
                index,
                slot,
                at,
            } => ShapeError::Inadmissible {
                form,
                index,
                slot,
                at,
            },
            ShapeError::DictDefault { site, at } => ShapeError::DictDefault { site, at },
            ShapeError::ClosedBucket { key, at } => ShapeError::ClosedBucket {
                key: self.copied(key),
                at,
            },
            ShapeError::NoKeyword { at } => ShapeError::NoKeyword { at },
            ShapeError::RankedDefinition { at } => ShapeError::RankedDefinition { at },
            ShapeError::NestedBinder { at } => ShapeError::NestedBinder { at },
            ShapeError::RankingDisagrees { key, at } => ShapeError::RankingDisagrees {
                key: self.copied(key),
                at,
            },
            ShapeError::NoCandidate { key, at } => ShapeError::NoCandidate {
                key: self.copied(key),
                at,
            },
            ShapeError::Overlaps { key, builtin, at } => ShapeError::Overlaps {
                key: self.copied(key),
                builtin,
                at,
            },
            ShapeError::NoAdmittingCandidate {
                key,
                arguments,
                listed,
                members,
                shapes,
                at,
            } => ShapeError::NoAdmittingCandidate {
                key: self.copied(key),
                arguments: self.copied(arguments),
                listed: self.copied(listed),
                members: self.copied(members),
                shapes: self.copied(shapes),
                at,
            },
            ShapeError::NoField {
                of,
                written,
                field,
                at,
            } => ShapeError::NoField {
                of,
                written,
                field,
                at,
            },
            ShapeError::NoMember { of, member, at } => ShapeError::NoMember { of, member, at },
            ShapeError::Unlisted { name, at } => ShapeError::Unlisted { name, at },
            ShapeError::OpenPathInType { at } => ShapeError::OpenPathInType { at },
            ShapeError::UnpinnedMember {
                member,
                parameter,
                at,
            } => ShapeError::UnpinnedMember {
                member,
                parameter,
                at,
            },
            ShapeError::Ambiguous {
                key,
                arguments,
                members,
                count,
                at,
            } => ShapeError::Ambiguous {
                key: self.copied(key),
                arguments: self.copied(arguments),
                members: self.copied(members),
                count,
                at,
            },
            ShapeError::ReturnNeverSatisfied { refused, at } => {
                ShapeError::ReturnNeverSatisfied { refused, at }
            }
            ShapeError::AscriptionNeverSatisfied { refused, at } => {
                ShapeError::AscriptionNeverSatisfied { refused, at }
            }
            ShapeError::ViewReveals {
                member,
                ascribed,
                at,
            } => ShapeError::ViewReveals {
                member,
                ascribed,
                at,
            },
            ShapeError::AnnotationNeverSatisfied { refused, at } => {
                ShapeError::AnnotationNeverSatisfied { refused, at }
            }
            ShapeError::CallNeverSatisfied {
                callee,
                rebounds,
                payload,
                parameters,
                at,
            } => ShapeError::CallNeverSatisfied {
                callee,
                rebounds: rebounds.laid(self.lay),
                payload,
                parameters,
                at,
            },
            ShapeError::ReadNeverSatisfied {
                container,
                argument,
                member,
                at,
            } => ShapeError::ReadNeverSatisfied {
                container,
                argument,
                member,
                at,
            },
            ShapeError::SpreadNeverAList { value, at } => {
                ShapeError::SpreadNeverAList { value, at }
            }
            ShapeError::SpliceNeverCode { value, at } => ShapeError::SpliceNeverCode { value, at },
            ShapeError::SpliceInKnot { at } => ShapeError::SpliceInKnot { at },
            ShapeError::NotAGuard { at } => ShapeError::NotAGuard { at },
            ShapeError::NoAdmittingArm {
                scrutinee,
                union,
                at,
            } => ShapeError::NoAdmittingArm {
                scrutinee,
                union,
                at,
            },
            ShapeError::AmbiguousArms {
                count,
                scrutinee,
                at,
            } => ShapeError::AmbiguousArms {
                count,
                scrutinee,
                at,
            },
            ShapeError::NotUnion {
                clause,
                written,
                at,
            } => ShapeError::NotUnion {
                clause,
                written,
                at,
            },
            ShapeError::NoVariant { label, union, at } => {
                ShapeError::NoVariant { label, union, at }
            }
            ShapeError::Uncovered { union, variant, at } => {
                ShapeError::Uncovered { union, variant, at }
            }
            ShapeError::LoneVariant { name, at } => ShapeError::LoneVariant { name, at },
            ShapeError::NotCode { refused, at } => ShapeError::NotCode { refused, at },
            ShapeError::BodyNotCode { refused, at } => ShapeError::BodyNotCode { refused, at },
            ShapeError::ParameterOutsideBody { name, at } => {
                ShapeError::ParameterOutsideBody { name, at }
            }
            ShapeError::NotAModule { refused, at } => ShapeError::NotAModule { refused, at },
            ShapeError::NotASignature { ascribed, at } => {
                ShapeError::NotASignature { ascribed, at }
            }
            ShapeError::EvalNeverSatisfied { refused, at } => {
                ShapeError::EvalNeverSatisfied { refused, at }
            }
            ShapeError::Type { error, at } => ShapeError::Type { error, at },
            ShapeError::UnderCall { call, refusal } => ShapeError::UnderCall {
                call,
                refusal: resident(self.lay, self.error(refusal)),
            },
            ShapeError::RepeatedGuard { guard, first, at } => ShapeError::RepeatedGuard {
                guard: match guard {
                    Repeated::Type(handle) => Repeated::Type(handle),
                    Repeated::Value(Scalar::Str(text)) => Repeated::Value(Scalar::Str(
                        std::str::from_utf8(self.copied(text.as_bytes()))
                            .expect("a copy of a string is one"),
                    )),
                    Repeated::Value(Scalar::Number(bits)) => Repeated::Value(Scalar::Number(bits)),
                    Repeated::Value(Scalar::Bool(truth)) => Repeated::Value(Scalar::Bool(truth)),
                    Repeated::Value(Scalar::Null) => Repeated::Value(Scalar::Null),
                    Repeated::Label(label) => Repeated::Label(label),
                    Repeated::Default => Repeated::Default,
                },
                first,
                at,
            },
            ShapeError::Grows { name, at } => ShapeError::Grows { name, at },
        }
    }
}
