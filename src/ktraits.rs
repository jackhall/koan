pub trait Parseable {
    fn equal(&self, other: &dyn Parseable) -> bool;
    fn summarize(&self) -> String;
}

pub trait Executable: Parseable {
    fn execute(&self, args: &[&dyn Parseable]) -> Box<dyn Parseable>;
}

pub trait Iterable: Parseable {
    fn iterate(&self) -> Vec<Box<dyn Parseable>>;
}

pub trait Collection: Iterable {
    fn contains(&self, key: &dyn Parseable) -> bool;
}

pub trait Serializable: Parseable {
    fn hash(&self) -> u64;
    fn encode(&self) -> Vec<u8>;
    fn decode(bytes: &[u8]) -> Self where Self: Sized;
}

pub trait Monadic {
    type Inner;
    type Wrap<T>: Monadic<Inner = T>;

    fn pure(inner: Self::Inner) -> Self;
    fn bind<B, F: Fn(Self::Inner) -> Self::Wrap<B>>(self, f: F) -> Self::Wrap<B>;
}
