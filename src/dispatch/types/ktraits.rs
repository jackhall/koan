use std::hash::{Hash, Hasher};

use super::ktype::KType;

/// Base trait for everything that participates in the language: values, expressions, and
/// functions all carry a canonical string `summarize` and a structural `equal`. Used widely as
/// `&dyn Parseable` for heterogeneous collections of language objects.
///
/// `ktype` returns the `KType` tag for this value. For containers (List, Dict) the impl walks
/// elements to project the parameterized type — see `KObject::ktype` for the semantics. For
/// dict-key types (`KKey`) the result is the appropriate scalar tag, used by `KObject::Dict`'s
/// `ktype` to infer `Dict<K, V>`.
pub trait Parseable {
    fn equal(&self, other: &dyn Parseable) -> bool;
    fn summarize(&self) -> String;
    fn ktype(&self) -> KType;
}

/// A `Parseable` that can be hashed and round-tripped through bytes. Doubles as the
/// `Dict` key trait — the `Hash`/`PartialEq`/`Eq` impls below for `dyn Serializable`
/// are what make `HashMap<Box<dyn Serializable>, _>` viable in `KObject::Dict`.
///
/// `clone_box` lets a boxed key be cloned without knowing its concrete type — required for
/// `KObject::Dict::deep_clone`. The returned box is `'static` (since concrete keys today are
/// owned-data types like `String`/`Number`); coerces into the Dict's `Box<dyn Serializable + 'a>`
/// slot via the standard `'static: 'a` trait-object covariance.
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
