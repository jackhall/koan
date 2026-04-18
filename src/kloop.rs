use crate::kobject::KObject;
use crate::ktraits::{Parseable, Iterable};
use std::collections::HashMap;

pub struct KLoop {
    pub base: KObject,
}

impl Parseable for KLoop {
    fn equal(&self, other: &dyn Parseable) -> bool { self.base.equal(other) }
    fn summarize(&self) -> String { self.base.summarize() }
}

impl Iterable for KLoop {
    fn iterate(&self) -> Vec<Box<dyn Parseable>> {
        self.base.remaining_args
            .keys()
            .map(|k| -> Box<dyn Parseable> {
                Box::new(KObject { name: k.clone(), remaining_args: HashMap::new() })
            })
            .collect()
    }
}
