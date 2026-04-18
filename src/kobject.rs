use std::collections::HashMap;
use crate::ktraits::Parseable;

/// A partially-applied object: holds a function and its remaining unbound args.
pub struct KObject {
    pub name: String,
    pub remaining_args: HashMap<String, Box<dyn Parseable>>,
}

impl Parseable for KObject {
    fn equal(&self, other: &dyn Parseable) -> bool {
        self.summarize() == other.summarize()
    }
    fn summarize(&self) -> String {
        self.name.clone()
    }
}
