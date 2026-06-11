//! **Feature-gated (`exec-v2`).** Overload matching as record subtyping. A function's parameters
//! *are* a record type, a call's resolved arguments *are* a record value, and "does this overload
//! accept these arguments" is exactly "is the argument record (type) more specific than the
//! parameter record type" — every parameter present and accepted, extra arguments allowed
//! (call-by-name width drop). Each argument contributes its *memoized* [`Carried::ktype`]; the
//! carried type is trusted and the value is never walked. Parallel to the live matcher; the
//! existing dispatcher is untouched.

use super::KFunction;
use crate::machine::model::types::{ExpressionSignature, Record, SignatureElement};
use crate::machine::model::values::Carried;
use crate::machine::model::KType;

/// The parameter record type `(name → declared type)`: the signature's `Argument`s. `Keyword`
/// elements are the call form's own literals and contribute no field.
pub fn param_record_type<'a>(signature: &ExpressionSignature<'a>) -> KType<'a> {
    let fields = signature.elements.iter().filter_map(|el| match el {
        SignatureElement::Argument(arg) => Some((arg.name.clone(), arg.ktype.clone())),
        SignatureElement::Keyword(_) => None,
    });
    KType::Record(Box::new(Record::from_pairs(fields)))
}

/// The argument record type `(name → carried type)`: each value's memoized `ktype`, never walked.
pub fn args_record_type<'a>(args: &Record<Carried<'a>>) -> KType<'a> {
    let fields = args
        .iter()
        .map(|(name, carried)| (name.clone(), carried.ktype()));
    KType::Record(Box::new(Record::from_pairs(fields)))
}

/// Does `signature` accept `args`? The match is record subtyping: the argument record type must be
/// more specific than *or equal to* the parameter record type (`is_more_specific_than` is strict —
/// the exact-match case is the equality). Every parameter present and accepted, extra arguments
/// allowed. Signature-only core, directly testable.
pub fn signature_accepts_args<'a>(
    signature: &ExpressionSignature<'a>,
    args: &Record<Carried<'a>>,
) -> bool {
    let args_type = args_record_type(args);
    let param_type = param_record_type(signature);
    args_type.is_more_specific_than(&param_type) || args_type == param_type
}

impl<'a> KFunction<'a> {
    /// Does this function accept `args` (overload match)? See [`signature_accepts_args`].
    pub fn signature_accepts(&self, args: &Record<Carried<'a>>) -> bool {
        signature_accepts_args(&self.signature, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::core::RuntimeArena;
    use crate::machine::model::types::{Argument, ReturnType};
    use crate::machine::model::values::KObject;

    /// `(DOUBLE x: Number)` — one keyword, one `Number` parameter.
    fn double_signature<'a>() -> ExpressionSignature<'a> {
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Number),
            elements: vec![
                SignatureElement::Keyword("DOUBLE".to_string()),
                SignatureElement::Argument(Argument {
                    name: "x".to_string(),
                    ktype: KType::Number,
                }),
            ],
        }
    }

    fn record_of<'a>(pairs: Vec<(&str, Carried<'a>)>) -> Record<Carried<'a>> {
        let mut record = Record::new();
        for (name, value) in pairs {
            record.insert(name.to_string(), value);
        }
        record
    }

    #[test]
    fn matching_arg_is_accepted() {
        let arena = RuntimeArena::new();
        let seven = Carried::Object(arena.alloc_object(KObject::Number(7.0)));
        assert!(signature_accepts_args(
            &double_signature(),
            &record_of(vec![("x", seven)])
        ));
    }

    #[test]
    fn extra_arg_is_accepted_width_drop() {
        let arena = RuntimeArena::new();
        let seven = Carried::Object(arena.alloc_object(KObject::Number(7.0)));
        let extra = Carried::Object(arena.alloc_object(KObject::KString("y".to_string())));
        assert!(signature_accepts_args(
            &double_signature(),
            &record_of(vec![("x", seven), ("y", extra)])
        ));
    }

    #[test]
    fn wrong_type_is_rejected() {
        let arena = RuntimeArena::new();
        let text = Carried::Object(arena.alloc_object(KObject::KString("nope".to_string())));
        assert!(!signature_accepts_args(
            &double_signature(),
            &record_of(vec![("x", text)])
        ));
    }

    #[test]
    fn missing_parameter_is_rejected() {
        assert!(!signature_accepts_args(&double_signature(), &Record::new()));
    }
}
