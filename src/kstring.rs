use crate::kobject::KObject;
use crate::ktraits::{Parseable, Serializable};
use std::collections::HashMap;

pub struct KString {
    pub base: KObject,
    pub value: String,
}

impl Parseable for KString {
    fn equal(&self, other: &dyn Parseable) -> bool { self.base.equal(other) }
    fn summarize(&self) -> String { self.value.clone() }
}

impl Serializable for KString {
    fn hash(&self) -> u64 {
        self.value.bytes().fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64))
    }
    fn encode(&self) -> Vec<u8> { self.value.as_bytes().to_vec() }
    fn decode(bytes: &[u8]) -> Self {
        let value = String::from_utf8_lossy(bytes).into_owned();
        KString {
            base: KObject { name: value.clone(), remaining_args: HashMap::new() },
            value,
        }
    }
}
