use super::ktype::KType;

/// Base trait for everything that participates in the language: values, expressions, and
/// functions carry a `ktype` that — for container values — walks elements to project the
/// parameterized type. Structural equality is not here: values compare via
/// [`KObject::value_equal`](super::super::values::KObject::value_equal).
///
/// Canonical-string rendering is an inherent `summarize` on each implementor rather than a
/// trait method: [`KObject`](super::super::values::KObject) renders the types it carries and so
/// needs a [`TypeRegistry`](super::TypeRegistry), while a `KExpression` and a `KKey` render
/// surface syntax alone and stay registry-free.
pub trait Parseable {
    fn ktype(&self) -> KType;
}
