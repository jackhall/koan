use std::hash::{Hash, Hasher};

use super::ktype::KType;

/// Base trait for everything that participates in the language: values, expressions, and
/// functions all carry a canonical string `summarize` and a structural `equal`.
///
/// For container values, `ktype` walks elements to project the parameterized type.
pub trait Parseable {
    fn equal(&self, other: &dyn Parseable) -> bool;
    fn summarize(&self) -> String;
    fn ktype(&self) -> KType;
}

/// A `Parseable` that can be hashed and round-tripped through bytes. Doubles as the
/// `Dict` key trait — the `Hash`/`PartialEq`/`Eq` impls below make
/// `HashMap<Box<dyn Serializable>, _>` viable.
///
/// `clone_box` lets a boxed key be cloned without knowing its concrete type. The returned
/// box is `'static` (concrete keys are owned-data types like `String`/`Number`) and coerces
/// into a `Box<dyn Serializable + 'a>` slot via `'static: 'a` trait-object covariance.
pub trait Serializable: Parseable {
    fn hash(&self, state: &mut dyn Hasher);
    fn encode(&self) -> Vec<u8>;
    fn decode(bytes: &[u8]) -> Self where Self: Sized;
    fn clone_box(&self) -> Box<dyn Serializable>;
}

impl<'a> Hash for dyn Serializable + 'a {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Serializable::hash(self, state);
    }
}

impl<'a> PartialEq for dyn Serializable + 'a {
    fn eq(&self, other: &Self) -> bool {
        self.equal(other)
    }
}

impl<'a> Eq for dyn Serializable + 'a {}
