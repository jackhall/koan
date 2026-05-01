use std::collections::HashMap;

use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::kfunction::{
    ArgumentBundle, KFunction, KType, SignatureElement, Specificity, UntypedKey,
};
use super::kobject::KObject;
use super::ktraits::Parseable;

/// A function call that has been resolved but not yet executed: the original parsed expression,
/// the chosen `KFunction`, and the `ArgumentBundle` produced by `KFunction::bind`. Carried
/// inside `KObject::KTask` and is the unit of deferred work in the dispatch system.
pub struct KFuture<'a> {
    pub parsed: KExpression<'a>,
    pub function: &'a KFunction<'a>,
    pub bundle: ArgumentBundle<'a>,
}

/// Lexical environment. `functions` buckets overloads by their *untyped signature* — the
/// arrangement of fixed tokens and slots with slot types erased — so dispatch can pick
/// between same-shape overloads by `KType` specificity. `out` is pluggable so tests and
/// embedders can capture builtin output instead of routing it to stdout.
pub struct Scope<'a> {
    pub outer: Option<&'a Scope<'a>>,
    pub data: HashMap<String, &'a KObject<'a>>,
    pub functions: HashMap<UntypedKey, Vec<&'a KFunction<'a>>>,
    pub out: Box<dyn std::io::Write + 'a>,
}

impl<'a> Scope<'a> {
    pub fn add(&mut self, name: String, obj: &'a KObject<'a>) {
        if let KObject::KFunction(f) = obj {
            let key = f.signature.untyped_key();
            self.functions.entry(key).or_default().push(*f);
        }
        self.data.insert(name, obj);
    }

    /// Look up `name` in this scope, walking the `outer` chain on miss. Returns the bound
    /// `KObject` from the nearest enclosing scope, or `None` if unbound at every level.
    pub fn lookup(&self, name: &str) -> Option<&'a KObject<'a>> {
        if let Some(obj) = self.data.get(name).copied() {
            return Some(obj);
        }
        self.outer.and_then(|outer| outer.lookup(name))
    }

    /// Resolve `expr` against this scope's functions, walking `outer` on miss so child scopes
    /// inherit from their parents. Ambiguity does *not* fall through to `outer` — the inner
    /// scope had a real conflict, and silently shadowing it would hide it from the author.
    pub fn dispatch(&self, expr: KExpression<'a>) -> Result<KFuture<'a>, String> {
        match self.pick(&expr) {
            Pick::One(f) => return f.bind(expr),
            Pick::Ambiguous(n) => {
                return Err(format!(
                    "ambiguous dispatch: {n} candidates match {} with equal specificity",
                    expr.summarize(),
                ));
            }
            Pick::None => {}
        }
        if let Some(outer) = self.outer {
            return outer.dispatch(expr);
        }
        Err(format!("no matching function for {}", expr.summarize()))
    }

    /// Find a "lazy candidate" for `expr`: a matching function with at least one
    /// `KType::KExpression` slot bound by an `ExpressionPart::Expression`. Returns the indices
    /// of the *eager* `Expression` parts — the caller schedules those as deps and leaves the
    /// lazy ones in place for the receiving builtin to dispatch itself. Walks `outer` like
    /// `dispatch` does.
    ///
    /// TODO(lazy-list-of-expressions): once user functions exist, `[e1 e2 e3]` will need to
    /// ride into the parent as `KExpression` data rather than be eagerly scheduled. Today
    /// every list-literal element resolves eagerly via `schedule_list_literal`.
    pub fn lazy_candidate(&self, expr: &KExpression<'_>) -> Option<Vec<usize>> {
        if !expr.parts.iter().any(|p| matches!(p, ExpressionPart::Expression(_))) {
            return None;
        }
        let mut viable: Vec<(&KFunction<'a>, Vec<usize>)> = self
            .functions
            .get(&expr.untyped_key())
            .into_iter()
            .flatten()
            .filter_map(|f| lazy_eager_indices(f, expr).map(|e| (*f, e)))
            .collect();
        if !viable.is_empty() {
            let funcs: Vec<&KFunction<'_>> = viable.iter().map(|(f, _)| *f).collect();
            // Ambiguous → return None and let `dispatch` surface the actual error at execute
            // time. Falling back to the eager pipeline here would misevaluate the lazy slot.
            return pick_most_specific_index(&funcs).map(|i| viable.swap_remove(i).1);
        }
        self.outer.and_then(|outer| outer.lazy_candidate(expr))
    }

    /// Internal: pick within this scope's own bucket only. Returns `None` if the bucket is
    /// missing or has no matching candidates; the caller decides whether to walk `outer`.
    fn pick(&self, expr: &KExpression<'a>) -> Pick<'a> {
        let key = expr.untyped_key();
        let bucket = match self.functions.get(&key) {
            Some(b) => b,
            None => return Pick::None,
        };
        let candidates: Vec<&'a KFunction<'a>> = bucket
            .iter()
            .filter(|f| f.signature.matches(expr))
            .copied()
            .collect();
        match pick_most_specific_index(&candidates) {
            Some(i) => Pick::One(candidates[i]),
            None if candidates.is_empty() => Pick::None,
            None => Pick::Ambiguous(candidates.len()),
        }
    }
}

enum Pick<'a> {
    One(&'a KFunction<'a>),
    Ambiguous(usize),
    None,
}

/// Pairwise specificity tournament: returns `Some(i)` iff `candidates[i]` is strictly more
/// specific than every other candidate. Returns `None` if the bucket is empty or if no
/// candidate dominates every peer (callers distinguish via `candidates.is_empty()`).
fn pick_most_specific_index(candidates: &[&KFunction<'_>]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .find(|(i, a)| {
            candidates.iter().enumerate().all(|(j, b)| {
                *i == j
                    || matches!(a.signature.specificity_vs(&b.signature), Specificity::StrictlyMore)
            })
        })
        .map(|(i, _)| i)
}

/// `lazy_candidate` shape check for a single function: is this a viable lazy match for `expr`,
/// and if so what are the indices of its eager-Expression parts? Returns `None` when the
/// function isn't a lazy candidate (length mismatch, fixed-token mismatch, no `KExpression`
/// slot binding an `Expression` part, or any other arg-type mismatch).
fn lazy_eager_indices(f: &KFunction<'_>, expr: &KExpression<'_>) -> Option<Vec<usize>> {
    let sig = &f.signature;
    if sig.elements.len() != expr.parts.len() {
        return None;
    }
    let mut eager_indices: Vec<usize> = Vec::new();
    let mut has_lazy_slot = false;
    for (i, (el, part)) in sig.elements.iter().zip(expr.parts.iter()).enumerate() {
        match (el, part) {
            (SignatureElement::Token(s), ExpressionPart::Token(t)) if s == t => {}
            (SignatureElement::Token(_), _) => return None,
            (SignatureElement::Argument(arg), part) => match (arg.ktype, part) {
                (KType::KExpression, ExpressionPart::Expression(_)) => {
                    has_lazy_slot = true;
                }
                (KType::KExpression, _) => return None,
                (_, ExpressionPart::Expression(_)) => {
                    // Speculative: assume the eager-evaluated result will type-match at late
                    // dispatch. If not, dispatch will fail at that point.
                    eager_indices.push(i);
                }
                (_, other) => {
                    if !arg.matches(other) {
                        return None;
                    }
                }
            },
        }
    }
    if has_lazy_slot { Some(eager_indices) } else { None }
}

#[cfg(test)]
impl<'a> Scope<'a> {
    /// Test-only constructor: a root scope with no bindings and a swallowing writer. Generic
    /// over `'a` so callers can chain it under a stack-borrowed outer scope without fighting
    /// `'static`.
    pub fn test_sink() -> Scope<'a> {
        Scope {
            outer: None,
            data: HashMap::new(),
            functions: HashMap::new(),
            out: Box::new(std::io::sink()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Scope;
    use crate::dispatch::builtins::default_scope;
    use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

    #[test]
    fn dispatch_walks_outer_chain_to_find_builtin() {
        // Parent owns the LET builtin; child has no functions of its own. Dispatching LET
        // against the child must climb to the parent.
        let outer = default_scope();
        let mut inner = Scope::test_sink();
        inner.outer = Some(&outer);

        let expr = KExpression {
            parts: vec![
                ExpressionPart::Token("LET".into()),
                ExpressionPart::Token("x".into()),
                ExpressionPart::Token("=".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };

        assert!(inner.dispatch(expr).is_ok(), "child scope should inherit LET from outer");
    }

    #[test]
    fn dispatch_with_no_outer_and_no_match_errors() {
        let scope = Scope::test_sink();
        let expr = KExpression {
            parts: vec![ExpressionPart::Token("nope".into())],
        };
        assert!(scope.dispatch(expr).is_err());
    }

    // --- specificity / bucketing / shadowing tests for the dispatch refactor ---

    use crate::dispatch::builtins::register_builtin;
    use crate::dispatch::kfunction::{
        Argument, ArgumentBundle, ExpressionSignature, KType, SignatureElement,
    };
    use crate::dispatch::kobject::KObject;

    // Sentinel-returning bodies. Each produces a distinct `KString` so a test can tell which
    // overload won dispatch. The explicit `'a` is needed so the leaked `&'static KObject<'static>`
    // marker coerces (covariantly) to `&'a KObject<'a>`.
    fn marker<'a>(s: &'static str) -> &'a KObject<'a> {
        Box::leak(Box::new(KObject::KString(s.into())))
    }

    fn body_identifier<'a>(_s: &mut Scope<'a>, _a: ArgumentBundle<'a>) -> &'a KObject<'a> { marker("identifier") }
    fn body_any<'a>(_s: &mut Scope<'a>, _a: ArgumentBundle<'a>) -> &'a KObject<'a> { marker("any") }
    fn body_number_any<'a>(_s: &mut Scope<'a>, _a: ArgumentBundle<'a>) -> &'a KObject<'a> { marker("number_any") }
    fn body_any_number<'a>(_s: &mut Scope<'a>, _a: ArgumentBundle<'a>) -> &'a KObject<'a> { marker("any_number") }
    fn body_inner_any<'a>(_s: &mut Scope<'a>, _a: ArgumentBundle<'a>) -> &'a KObject<'a> { marker("inner_any") }
    fn body_outer_number<'a>(_s: &mut Scope<'a>, _a: ArgumentBundle<'a>) -> &'a KObject<'a> { marker("outer_number") }
    fn body_lowercase<'a>(_s: &mut Scope<'a>, _a: ArgumentBundle<'a>) -> &'a KObject<'a> { marker("lowercase") }

    fn one_slot_sig(name: &str, kt: KType) -> ExpressionSignature {
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![SignatureElement::Argument(Argument {
                name: name.into(),
                ktype: kt,
                
            })],
        }
    }

    /// `<a:A> OP <b:B>` — a binary-operator shape that includes a fixed token so the
    /// expression doesn't get caught by list-shape detection (which would treat any
    /// fixed-token-free multi-part expression as a list construction).
    fn two_slot_sig(a: KType, b: KType) -> ExpressionSignature {
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Argument(Argument {
                    name: "a".into(),
                    ktype: a,
                    
                }),
                SignatureElement::Token("OP".into()),
                SignatureElement::Argument(Argument {
                    name: "b".into(),
                    ktype: b,
                    
                }),
            ],
        }
    }

    /// Register the `Identifier` overload AFTER the `Any` overload (the opposite of
    /// `default_scope`'s declaration order). Specificity-based dispatch should still pick
    /// `Identifier` for an identifier-shaped input.
    #[test]
    fn dispatch_picks_identifier_over_any_regardless_of_registration_order() {
        let mut scope = Scope::<'static>::test_sink();
        // Register Any first, then Identifier — reversed from default_scope.
        register_builtin(&mut scope, "any_first", one_slot_sig("v", KType::Any), body_any);
        register_builtin(&mut scope, "ident_second", one_slot_sig("v", KType::Identifier), body_identifier);

        let expr = KExpression { parts: vec![ExpressionPart::Token("foo".into())] };
        let future = scope.dispatch(expr).expect("should match Identifier overload");
        let result = (future.function.body)(&mut scope, future.bundle);
        assert!(matches!(result, KObject::KString(s) if s == "identifier"),
            "Identifier overload should win on an identifier input, got {:?}", summarize_marker(result));
    }

    /// Inner scope's `Any` overload shadows the outer scope's more-specific `Number` overload.
    /// Pure lexical shadowing — innermost match wins regardless of specificity at outer levels.
    #[test]
    fn dispatch_inner_scope_shadows_outer_more_specific() {
        // `register_builtin` requires `&mut Scope<'static>`, and `inner.outer` needs a
        // `&'static Scope<'static>`. Leak the outer scope to satisfy both: build it with
        // its function registered, then `Box::leak` it to acquire a `'static` borrow.
        let outer_ref: &'static Scope<'static> = {
            let mut outer = Scope::<'static>::test_sink();
            register_builtin(&mut outer, "outer_specific", one_slot_sig("v", KType::Number), body_outer_number);
            Box::leak(Box::new(outer))
        };

        let mut inner = Scope::<'static>::test_sink();
        register_builtin(&mut inner, "inner_loose", one_slot_sig("v", KType::Any), body_inner_any);
        inner.outer = Some(outer_ref);

        let expr = KExpression { parts: vec![ExpressionPart::Literal(KLiteral::Number(7.0))] };
        let future = inner.dispatch(expr).expect("inner Any should match");
        let result = (future.function.body)(&mut inner, future.bundle);
        assert!(matches!(result, KObject::KString(s) if s == "inner_any"),
            "inner Any must shadow outer Number (lexical shadowing > specificity), got {:?}",
            summarize_marker(result));
    }

    /// `<Number> OP <Any>` and `<Any> OP <Number>` are incomparable for an input matching
    /// both (`5 OP 7`): each is more specific in one slot and less in the other. Dispatch
    /// must error rather than silently picking one. The fixed `OP` token keeps the
    /// expression out of the list-shape short-circuit.
    #[test]
    fn dispatch_errors_on_ambiguous_overlap() {
        let mut scope = Scope::<'static>::test_sink();
        register_builtin(&mut scope, "number_any", two_slot_sig(KType::Number, KType::Any), body_number_any);
        register_builtin(&mut scope, "any_number", two_slot_sig(KType::Any, KType::Number), body_any_number);

        let expr = KExpression {
            parts: vec![
                ExpressionPart::Literal(KLiteral::Number(5.0)),
                ExpressionPart::Token("OP".into()),
                ExpressionPart::Literal(KLiteral::Number(7.0)),
            ],
        };
        let result = scope.dispatch(expr);
        match result {
            Err(e) => assert!(e.contains("ambiguous"), "expected ambiguity error, got: {e}"),
            Ok(_) => panic!("equally-specific overloads should produce an ambiguity error"),
        }
    }

    /// A lowercase fixed token in a registered signature is coerced to uppercase, so
    /// dispatching the uppercase form from a source program still hits the registered
    /// function. (Once monadic effects exist, this should also produce a warning effect.)
    #[test]
    fn registration_coerces_lowercase_fixed_tokens_to_uppercase() {
        let mut scope = Scope::<'static>::test_sink();
        let sig = ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Token("foo".into()), // lowercase — should be coerced
                SignatureElement::Argument(Argument {
                    name: "v".into(),
                    ktype: KType::Number,
                    
                }),
            ],
        };
        register_builtin(&mut scope, "FOO", sig, body_lowercase);

        // The source-side caller writes `FOO 1` (uppercase), which must match the coerced
        // `FOO <v>` registration.
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Token("FOO".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };
        let future = scope.dispatch(expr).expect("uppercase form should match coerced signature");
        let result = (future.function.body)(&mut scope, future.bundle);
        assert!(matches!(result, KObject::KString(s) if s == "lowercase"));
    }

    fn summarize_marker(obj: &KObject<'_>) -> String {
        match obj {
            KObject::KString(s) => s.clone(),
            KObject::Null => "null".into(),
            _ => "<other>".into(),
        }
    }
}
