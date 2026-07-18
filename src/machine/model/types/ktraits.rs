use super::ktype::KType;

/// Base trait for everything that participates in the language: values, expressions, and
/// functions all carry a canonical string `summarize`, and — for container values — a
/// `ktype` that walks elements to project the parameterized type. Structural equality is not
/// here: values compare via [`KObject::value_equal`](super::super::values::KObject::value_equal).
pub trait Parseable {
    fn summarize(&self) -> String;
    fn ktype(&self) -> KType;
}
