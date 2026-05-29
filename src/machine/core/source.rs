//! Source-file registry and span/location primitives. Owns the thread-local
//! `SOURCES` vector keyed by `FileId`, the active `CURRENT_FILE` for the parse
//! pass, and the wrapper types (`Span`, `Spanned`) that thread byte-offset
//! metadata through the AST.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Index into the thread-local `SOURCES` registry.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileId(pub u32);

/// Byte-offset half-open range into a `SourceFile.text`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

/// 1-based source location for diagnostic rendering. `col` follows LSP convention:
/// 1-based UTF-16 code unit count from the line start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLoc {
    pub path: Rc<str>,
    pub line: u32,
    pub col_utf16: u32,
}

/// One registered source. `line_starts` is built once at construction so `resolve`
/// is a binary search plus a per-line UTF-16 char count.
pub struct SourceFile {
    pub path: Rc<str>,
    pub text: String,
    line_starts: Vec<u32>,
}

impl SourceFile {
    pub fn new(path: impl Into<Rc<str>>, text: String) -> Self {
        let mut line_starts = Vec::with_capacity(text.len() / 32 + 1);
        line_starts.push(0);
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push((i as u32) + 1);
            }
        }
        SourceFile { path: path.into(), text, line_starts }
    }

    /// Resolve a byte offset into `(line, col_utf16)`, both 1-based.
    pub fn resolve(&self, offset: u32) -> (u32, u32) {
        let idx = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let line_start = self.line_starts[idx] as usize;
        let end = (offset as usize).min(self.text.len());
        let col_utf16 = self.text[line_start..end].encode_utf16().count() as u32 + 1;
        ((idx as u32) + 1, col_utf16)
    }
}

thread_local! {
    static SOURCES: RefCell<Vec<Rc<SourceFile>>> = const { RefCell::new(Vec::new()) };
    static CURRENT_FILE: Cell<Option<FileId>> = const { Cell::new(None) };
}

/// Push a source onto the thread-local registry. The returned `FileId` is the
/// index of the new entry.
pub fn register(file: SourceFile) -> FileId {
    SOURCES.with(|s| {
        let mut v = s.borrow_mut();
        let id = FileId(v.len() as u32);
        v.push(Rc::new(file));
        id
    })
}

/// Borrow the registered `SourceFile` for the duration of `f`. Panics if `id`
/// is out of range — the registry is append-only so the only way to hit that
/// is to forge a `FileId`, which the public API never returns.
pub fn with<R>(id: FileId, f: impl FnOnce(&SourceFile) -> R) -> R {
    SOURCES.with(|s| {
        let v = s.borrow();
        let file = v
            .get(id.0 as usize)
            .expect("FileId out of range — was the registry mutated outside `register`?")
            .clone();
        drop(v);
        f(&file)
    })
}

/// Active source for parse-side error attribution. Read by `KError::parse`.
pub fn current() -> Option<FileId> {
    CURRENT_FILE.with(Cell::get)
}

/// Swap the active source and return the previous value. Prefer `CurrentFileGuard`
/// over calling this directly so a panic mid-parse doesn't strand the slot.
pub fn set_current(id: Option<FileId>) -> Option<FileId> {
    CURRENT_FILE.with(|c| c.replace(id))
}

/// RAII handle: sets `CURRENT_FILE` on construction, restores the previous value
/// on drop. Required because a parse-pass panic otherwise leaves `CURRENT_FILE`
/// pointing at a stale id for the rest of the thread's lifetime, silently
/// misattributing later parses (and later in-language errors that consult it).
pub struct CurrentFileGuard {
    prev: Option<FileId>,
}

impl CurrentFileGuard {
    pub fn push(id: FileId) -> Self {
        Self { prev: set_current(Some(id)) }
    }
}

impl Drop for CurrentFileGuard {
    fn drop(&mut self) {
        set_current(self.prev.take());
    }
}

/// Wraps an AST node with optional span metadata. Used for `ExpressionPart`;
/// the enclosing `KExpression` keeps span as a direct field instead.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Option<Span>,
}

impl<T> Spanned<T> {
    pub fn bare(value: T) -> Self {
        Self { value, span: None }
    }

    pub fn at(value: T, span: Span) -> Self {
        Self { value, span: Some(span) }
    }
}

impl<T> From<T> for Spanned<T> {
    fn from(value: T) -> Self {
        Spanned::bare(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_first_line() {
        let f = SourceFile::new("<t>", "hello world".to_string());
        assert_eq!(f.resolve(0), (1, 1));
        assert_eq!(f.resolve(6), (1, 7));
    }

    #[test]
    fn resolve_multi_line() {
        let f = SourceFile::new("<t>", "ab\ncd\nef".to_string());
        assert_eq!(f.resolve(0), (1, 1));
        assert_eq!(f.resolve(3), (2, 1));
        assert_eq!(f.resolve(4), (2, 2));
        assert_eq!(f.resolve(6), (3, 1));
    }

    #[test]
    fn resolve_utf16_columns_count_code_units_not_bytes() {
        // "é" is 2 bytes in UTF-8 and 1 UTF-16 code unit. "💡" (U+1F4A1) is
        // 4 bytes in UTF-8 and 2 UTF-16 code units (surrogate pair).
        let f = SourceFile::new("<t>", "é💡x".to_string());
        assert_eq!(f.resolve(0), (1, 1));
        // After "é" (2 bytes): 1 UTF-16 code unit consumed, col = 2.
        assert_eq!(f.resolve(2), (1, 2));
        // After "é💡" (6 bytes): 1 + 2 UTF-16 code units, col = 4.
        assert_eq!(f.resolve(6), (1, 4));
    }

    #[test]
    fn register_and_with_round_trip() {
        let id = register(SourceFile::new("<a>", "abc".to_string()));
        let path = with(id, |f| f.path.clone());
        assert_eq!(&*path, "<a>");
    }

    #[test]
    fn guard_restores_previous_current_file() {
        let outer = register(SourceFile::new("<outer>", String::new()));
        let inner = register(SourceFile::new("<inner>", String::new()));
        let _o = CurrentFileGuard::push(outer);
        assert_eq!(current(), Some(outer));
        {
            let _i = CurrentFileGuard::push(inner);
            assert_eq!(current(), Some(inner));
        }
        assert_eq!(current(), Some(outer));
    }

    #[test]
    fn spanned_bare_has_no_span() {
        let s: Spanned<u32> = Spanned::bare(7);
        assert_eq!(s.value, 7);
        assert!(s.span.is_none());
    }

    #[test]
    fn spanned_from_wraps_with_none_span() {
        let s: Spanned<u32> = 9.into();
        assert!(s.span.is_none());
    }
}
