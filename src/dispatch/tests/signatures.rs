//! A module retyped at its slot: a signature-typed parameter, a declared return, an annotation, a
//! record field, `:!`. A member read through a module retyped to a signature sees exactly what the
//! signature declares, so the load reads it exactly and the run retypes the member to it.

use super::ascription::ends;
use super::run;
use super::statics::{body, loaded};

/// `Holder`, and `held`, whose `r` holds a field `Holder` hides.
const HOLDER: &str = "SIG Holder = #[(VAL r :{x :Number})]\n\
                      MODULE held = (LET r = {x = 1, y = \"hidden\"})\n";

/// `Counter`, and `ints`, which fits it at `Number`.
const COUNTER: &str = "SIG Counter FOR ALL #[Carrier] = \
                       #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                       MODULE ints = ((LET zero = 0) \
                       (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n";

/// Assert `program`, appended to [`HOLDER`], is refused at load for reading the hidden field.
fn refuses_the_hidden_field(program: &str) {
    let printed = run(&format!("{HOLDER}{program}"));
    assert!(
        printed.starts_with("load:") && printed.contains(":{x :Number} has no field y"),
        "{program}: {printed}"
    );
}

#[test]
fn a_hidden_field_of_a_member_read_through_a_signature_refuses_the_load() {
    refuses_the_hidden_field(
        "LET read = (FN :{h :Holder} -> Any = #(ATTR (h.r) y))\nPRINT (read {h = held})",
    );
    refuses_the_hidden_field(
        "LET give = (FN :{} -> Holder = #(held))\nPRINT (ATTR ((give {}).r) y)",
    );
    refuses_the_hidden_field("LET h :Holder = held\nPRINT (ATTR (h.r) y)");
    refuses_the_hidden_field(
        "LET read = (FN :{b :{inner :Holder}} -> Any = #(ATTR (b.inner.r) y))\n\
         PRINT (read {b = {inner = held}})",
    );
    refuses_the_hidden_field("PRINT (ATTR ((held :! Holder).r) y)");
}

#[test]
fn a_member_read_through_a_signature_typed_parameter_is_exactly_its_declared_type() {
    let source = format!(
        "{HOLDER}LET read = (FN :{{h :Holder}} -> Any = #(\n  \
         LET got = h.r\n  \
         got\n\
         ))\n\
         PRINT (read {{h = held}})"
    );
    loaded(&source, |program| {
        let read = body(program, program.shape(), "read");
        let exact = ":{x :Number}".to_string();
        assert_eq!(ends(program, read, "got"), (exact.clone(), exact));
    });
    assert_eq!(run(&source), "{x = 1}");
}

/// A member declared at a head parameter the slot leaves unpinned reads at the type the module
/// fits the signature at, which each run pins afresh.
#[test]
fn a_member_at_an_unpinned_parameter_reads_at_what_the_module_fits() {
    let source = format!(
        "{COUNTER}LET bump = (FN :{{m :Counter}} -> Any = #(m.succ {{x = m.zero}}))\n\
         PRINT (bump {{m = ints}})"
    );
    assert_eq!(run(&source), "1");
}

#[test]
fn an_overload_over_a_hidden_field_is_never_selected_through_a_signature() {
    let source = format!(
        "{HOLDER}EXPR #(PICK r :{{x :Number, y :Str}}) -> Str = #(\"wide\")\n\
         EXPR #(PICK r :{{x :Number}}) -> Str = #(\"narrow\")\n\
         LET pick = (FN :{{h :Holder}} -> Str = #(PICK h.r))\n\
         PRINT (pick {{h = held}})"
    );
    assert_eq!(run(&source), "narrow");
}

/// `USING` surfaces the members the module's carried signature names, at the types it declares.
#[test]
fn a_using_block_over_a_retyped_module_surfaces_what_its_signature_names() {
    let source = format!(
        "{HOLDER}MODULE wide = ((LET r = {{x = 1, y = \"s\"}}) (LET extra = 2))\n\
         LET h :Holder = wide\n\
         PRINT (USING h SCOPE (r))"
    );
    assert_eq!(run(&source), "{x = 1}");
}

/// A nested module declared at an application pinning its parameter reads the pin as a member,
/// though the module itself binds none.
#[test]
fn a_nested_module_read_through_a_header_reads_its_pinned_parameter() {
    let source = "SIG Inner FOR ALL #[Elem] = #[(VAL zero :Elem)]\n\
                  SIG Outer = #[(VAL inner :(Inner WITH {Elem = Number}))]\n\
                  MODULE m = (MODULE inner = (LET zero = 1))\n\
                  LET v = (m :! Outer)\n\
                  LET i = v.inner\n\
                  PRINT i.Elem\n\
                  PRINT v.inner.zero";
    assert_eq!(run(source), "Number\n1");
}

/// Two modules over one body, branded only by what they capture, stay apart through a header that
/// hides the brand, and so do their views' carriers.
#[test]
fn a_header_keeps_the_brand_its_signature_hides() {
    let source = "SIG Plain = #[(VAL zero :Number)]\n\
                  SIG Ord FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
                  LET metric = 1\nLET imperial = 2\n\
                  MODULE meters OVER #[metric] = (LET zero = 0)\n\
                  MODULE feet OVER #[imperial] = (LET zero = 0)\n\
                  LET m = (meters :! Plain)\nLET f = (feet :! Plain)\n\
                  PRINT ((m :| Ord).Carrier == (f :| Ord).Carrier)";
    assert_eq!(run(source), "false");
}

/// A view through a header is the view of its member: `m :| Counter` over `ints` passed through
/// `m :Counter` shares `ints :| Counter`'s carrier, so a value one seals passes the other's
/// `succ`, while a header pinning the parameter otherwise fits otherwise, and mints its own.
#[test]
fn a_view_through_a_header_is_the_view_of_its_member() {
    let source = format!(
        "{COUNTER}LET outside = (ints :| Counter)\n\
         LET inside = (FN :{{m :Counter}} -> Any = #(m :| Counter))\n\
         LET within = (inside {{m = ints}})\n\
         PRINT (within.succ {{x = outside.zero}})\n\
         PRINT (outside.succ {{x = within.zero}})"
    );
    assert_eq!(run(&source), "Carrier(1)\nCarrier(1)");
    let widened = "SIG Zeroed FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
                   MODULE ints = (LET zero = 0)\n\
                   LET plain = (ints :| Zeroed)\n\
                   LET wide = ((ints :! (Zeroed WITH {Carrier = (Number | Str)})) :| Zeroed)\n\
                   PRINT (wide.Carrier == plain.Carrier)";
    assert_eq!(run(widened), "false");
}

/// A function member of an opaque view returns a value retyped to the return its caller sees, so a
/// field the view's signature hides is no field of it.
#[test]
fn a_barriers_value_hides_what_its_return_hides() {
    let source = "SIG Counter FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL show :(FN :{} -> :{x :Number}))]\n\
                  MODULE ints = ((LET zero = 0) \
                  (LET show = (FN :{} -> :{x :Number, y :Str} = #({x = 1, y = \"leak\"}))))\n\
                  LET v = (ints :| Counter)\n\
                  LET got = (v.show {})\n";
    assert_eq!(run(&format!("{source}PRINT got")), "{x = 1}");
    let printed = run(&format!("{source}PRINT (ATTR got y)"));
    assert!(
        printed.starts_with("load:") && printed.contains(":{x :Number} has no field y"),
        "{printed}"
    );
}

/// A quantified member read through a signature declaring it a plain function type is a header
/// at that type, and a call through it solves the member's own group.
#[test]
fn a_quantified_member_read_at_a_plain_function_type_runs() {
    let source = "SIG Ider = #[(VAL f :(FN :{x :Number} -> Number))]\n\
                  MODULE m = (LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x)))\n\
                  LET call = (FN :{h :Ider} -> Number = #(h.f {x = 2}))\n\
                  PRINT (call {h = m})\n\
                  LET h :Ider = m\nLET g = h.f\nPRINT (g {x = 3})";
    assert_eq!(run(source), "2\n3");
}

/// A quantified member read through a signature declaring it at another scheme, at an instance
/// site, is a header at that scheme's instance over the same member.
#[test]
fn a_quantified_member_read_at_another_scheme_is_instantiated_under_its_header() {
    let source = "SIG Ider = #[(VAL f :(FN FOR ALL #[Elt] :{x :Elt, y :Elt} -> Elt))]\n\
                  MODULE m = (LET f = (FN FOR ALL #[Aa Bb] :{x :Aa, y :Bb} -> Aa = #(x)))\n\
                  LET h :Ider = m\n\
                  LET g :(FN :{x :Number, y :Number} -> Number) = h.f\n\
                  PRINT g\nPRINT (g {x = 1, y = 2})\nPRINT (h.f {x = 5, y = 6})\n\
                  PRINT (USING h SCOPE (f {x = 9, y = 9}))";
    assert_eq!(
        run(source),
        ":(FN :{x :Number y :Number} -> Number)\n1\n5\n9"
    );
}

/// `two`, with three overloads at `PICK`, and `Picks`, declaring one of them.
const TWO: &str = "SIG Picks = #[(EXPR #(PICK x :Number) -> Str)]\n\
                   MODULE two = ((EXPR #(PICK x :Number) -> Str = #(\"number\")) \
                   (EXPR #(PICK x :Str) -> Str = #(\"str\")) \
                   (EXPR #(PICK x :Any) -> Str = #(\"any\")))\n";

/// Through a header, a keyworded member offers the module's most specific overload covering it,
/// so a call its declared shape refuses is refused at load.
#[test]
fn a_keyworded_member_through_a_header_offers_its_most_specific_covering_overload() {
    assert_eq!(
        run(&format!("{TWO}PRINT (USING (two :! Picks) SCOPE (PICK 1))")),
        "number"
    );
    for refused in [
        "PRINT (USING (two :! Picks) SCOPE (PICK \"s\"))",
        "PRINT (USING (two :| Picks) SCOPE (PICK \"s\"))",
        "LET h :Picks = two\nPRINT (USING h SCOPE (PICK null))",
    ] {
        let printed = run(&format!("{TWO}{refused}"));
        assert!(
            printed.starts_with("load:") && printed.contains("no overload of `PICK _`"),
            "{refused}: {printed}"
        );
    }
    // An overload admitting more than the member is retyped to it, so the load refuses what the
    // member refuses even where nothing more specific covers it.
    let any = "SIG Picks = #[(EXPR #(PICK x :Number) -> Str)]\n\
               MODULE one = (EXPR #(PICK x :Any) -> Str = #(\"any\"))\n\
               LET h :Picks = one\n";
    let printed = run(&format!("{any}PRINT (USING h SCOPE (PICK \"s\"))"));
    assert!(printed.starts_with("load:"), "{printed}");
    assert_eq!(run(&format!("{any}PRINT (USING h SCOPE (PICK 1))")), "any");
    let through = format!(
        "{TWO}LET pick = (FN :{{m :Picks}} -> Str = #(USING (m :! Picks) SCOPE (PICK 2)))\n\
         PRINT (pick {{m = two}})"
    );
    assert_eq!(run(&through), "number");
}

/// Where no overload covering a keyworded member ranks first, a header offers every one, and a
/// call meeting them is ambiguous.
#[test]
fn a_keyworded_member_two_overloads_cover_alike_is_ambiguous_through_a_header() {
    let source = "SIG Picks = #[(EXPR #(PICK x :Number) -> Str)]\n\
                  MODULE both = ((EXPR #(PICK x :(Number | Str)) -> Str = #(\"a\")) \
                  (EXPR #(PICK x :(Number | Bool)) -> Str = #(\"b\")))\n\
                  PRINT (USING (both :! Picks) SCOPE (PICK 1))";
    assert_eq!(
        run(source),
        "error: ambiguous call of `PICK _`: 2 overloads admit (Number) and none ranks first"
    );
}

/// A keyworded member at a quantified shape, called through a header or a view's barrier, returns
/// a value retyped to the declared return under the call's solution, so a field it hides is no
/// field of it.
#[test]
fn a_quantified_keyworded_member_returns_what_its_declared_shape_returns() {
    let source = "SIG Getter = #[(EXPR FOR ALL #[Elt] #(GET x :Elt) -> :{x :Elt})]\n\
                  SIG Viewed FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (EXPR FOR ALL #[Elt] #(GET x :Elt) -> :{x :Elt})]\n\
                  MODULE m = ((LET zero = 0) \
                  (EXPR FOR ALL #[Elt] #(GET x :Elt) -> :{x :Elt, y :Str} = #({x = x, y = \"leak\"})))\n\
                  LET h :Getter = m\nLET v = (m :| Viewed)\n";
    for through in ["h", "v"] {
        assert_eq!(
            run(&format!("{source}PRINT (USING {through} SCOPE (GET 1))")),
            "{x = 1}",
            "{through}"
        );
        let printed = run(&format!(
            "{source}LET got :Any = (USING {through} SCOPE (GET 1))\nPRINT (ATTR got y)"
        ));
        assert!(
            printed.starts_with("error:") && printed.contains("has no field y"),
            "{through}: {printed}"
        );
    }
    // A view's keyworded member reads at most its barrier's return at load, as an unquantified one
    // does; through a header the load reads it exactly.
    let printed = run(&format!("{source}PRINT (USING h SCOPE (ATTR (GET 1) y))"));
    assert!(
        printed.starts_with("load:") && printed.contains(":{x :Number} has no field y"),
        "{printed}"
    );
}

#[test]
fn a_view_over_a_closed_tree_reads_its_picks() {
    // The load sees all of `both`, so it records which of its overloads cover `PICK`, ranked:
    // the view the run builds takes them by position and covers nothing itself.
    use crate::knot::module::view::COVERED;
    let source = "SIG Picks = #[(EXPR #(PICK x :Number) -> Str)]\n\
                  MODULE both = ((EXPR #(PICK x :(Number | Str)) -> Str = #(\"a\")) \
                  (EXPR #(PICK x :Number) -> Str = #(\"n\")) \
                  (EXPR #(PICK x :Str) -> Str = #(\"s\")))\n\
                  PRINT (USING (both :| Picks) SCOPE (PICK 1))";
    COVERED.with(|covered| covered.set(0));
    assert_eq!(run(source), "n");
    assert_eq!(COVERED.with(std::cell::Cell::get), 0);
}
