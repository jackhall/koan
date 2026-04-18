use crate::kobject::KObject;
use crate::ktraits::{Parseable, Iterable, Collection};
use std::collections::HashMap;

pub struct KList {
    pub base: KObject,
    pub items: Vec<String>,
}

impl Parseable for KList {
    fn equal(&self, other: &dyn Parseable) -> bool { self.base.equal(other) }
    fn summarize(&self) -> String { self.base.summarize() }
}

impl Iterable for KList {
    fn iterate(&self) -> Vec<Box<dyn Parseable>> {
        self.items.iter()
            .map(|s| -> Box<dyn Parseable> {
                Box::new(KObject { name: s.clone(), remaining_args: HashMap::new() })
            })
            .collect()
    }
}

impl Collection for KList {
    fn contains(&self, key: &dyn Parseable) -> bool {
        self.items.iter().any(|s| s == &key.summarize())
    }
}
