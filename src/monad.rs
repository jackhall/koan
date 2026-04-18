use crate::ktraits::Monadic;

impl<A> Monadic for Option<A> {
    type Inner = A;
    type Wrap<T> = Option<T>;

    fn pure(inner: A) -> Self { Some(inner) }
    fn bind<B, F: Fn(A) -> Option<B>>(self, f: F) -> Option<B> { self.and_then(f) }
}
