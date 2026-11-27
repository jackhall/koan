//! A spelling over a run-bound name is licensed by the name's bound where the program loads, so no
//! binding refuses a spelling
//! ([what a body may ask of a `FOR ALL` name](../../../design/quantified-types.md#what-a-body-may-ask-of-a-for-all-name)):
//! a field projection is the lattice's projection, a type declared over the name shows what its
//! declaration writes, and every other spelling over the name is refused at load.

use super::run;

const MAYBE: &str = "UNION Maybe = #{Some: Number, None: Null}";
const OPTION: &str = "UNION (Elem -> Option) = #{Some: Elem, None: Null}";

/// `source`'s load refusal; panics where it loads.
fn refused(source: &str) -> String {
    let ran = run(source);
    assert!(
        ran.starts_with("load: "),
        "`{source}` is refused at load: {ran}"
    );
    ran
}

/// A field projection off a `FOR ALL` name whose bound declares the field reads each binding's
/// field, a union's the union of its members', and agrees with the value read of the field.
#[test]
fn a_projection_off_a_for_all_name_is_licensed_by_its_bound() {
    let sizeof = "EXPR FOR ALL #{Elt: :{size :Any}} #(SIZEOF x :Elt) -> Any = #(\n  \
                  LET n :(Elt.size) = (x.size)\n  \
                  n)\n\
                  EXPR #(GO y :(:{size :Number} | :{size :Str})) -> Any = #(SIZEOF y)\n\
                  PRINT (GO {size = 5})";
    assert_eq!(run(sizeof), "5");
    let resizer = "EXPR FOR ALL #{Elt: :{size :Any}} #(RESIZER x :Elt) -> Any = #(\n  \
                   (FN :{n :(Elt.size)} -> Elt = #(x)))\n\
                   LET grow = (RESIZER {size = 5, width = 2})\n\
                   PRINT (grow {n = 6})";
    assert_eq!(run(resizer), "{size = 5, width = 2}");
    let closed = "LET n :((:{size :Number} | :{size :Str}).size) = \"big\"\nPRINT n";
    assert_eq!(
        run(closed),
        "big",
        "a union of record types projects to its fields' union"
    );
}

/// Each spelling over a `FOR ALL` name its bound does not license is refused where the program
/// loads, worded over the bound, and through the bound's own spelling where it has one.
#[test]
fn a_spelling_its_bound_does_not_license_is_refused_at_load() {
    let body = |bound: &str, spelled: &str| {
        format!(
            "{MAYBE}\n{OPTION}\nSIG Holds FOR ALL #[Car] = #[(VAL c :Car)]\n\
             EXPR FOR ALL #{{Elt: {bound}}} #(SUBJECT x :Elt) -> Any = #(\n  {spelled}\n)"
        )
    };
    let cases = [
        (
            "Any",
            "LET n :(Elt.size) = 1",
            "Elt has no member size: its bound Any declares none",
        ),
        (
            ":{width :Number}",
            "LET n :(Elt.size) = 1",
            "its bound :{width :Number} declares none",
        ),
        ("Maybe", "LET n :(Elt.None) = 1", "spell it Maybe.None"),
        (
            "Maybe",
            "MATCH x UNDER Elt WITH #{Some: (it), None: (0)}",
            "spell the clause UNDER Maybe",
        ),
        (
            "Any",
            "MATCH x UNDER Elt WITH #{_: (0)}",
            "its bound Any is no union",
        ),
        ("Option", "LET n :(Number -> Elt) = 1", "so it is no head"),
        (
            "Option",
            "LET n :(Elt {Elem = Number}) = 1",
            "so it is no head",
        ),
        (
            "Holds",
            "LET n :(Elt WITH {Car = Number}) = 1",
            "so it is no head",
        ),
        ("Any", "LET n :(Elt NEEDING #[y]) = 1", "so it is no head"),
        ("Any", "LET n :(UNBUILT Elt) = 1", "so it is no head"),
    ];
    for (bound, spelled, says) in cases {
        let refusal = refused(&body(bound, spelled));
        assert!(
            refusal.contains(says),
            "`{spelled}` under `{bound}`: {refusal}"
        );
    }
}

/// A name with no declared bound licenses nothing: a projection off a `:Type` parameter, and a
/// `MATCH … UNDER` over a path through a head parameter its signature leaves unpinned.
#[test]
fn a_name_with_no_declared_bound_licenses_nothing() {
    refused("EXPR #(PROJ k :Type) -> Any = #(\n  LET n :(k.size) = 1\n  n)");
    let refusal = refused(
        "SIG Shaped FOR ALL #[Shape] = #[]\n\
         EXPR #(OPEN v :Any m :Shaped) -> Any = #(\n  MATCH v UNDER m.Shape WITH #{_: (0)})",
    );
    assert!(
        refusal.contains("takes a union, and Shape is not one"),
        "{refusal}"
    );
}

/// A `MATCH … UNDER`'s arms are checked once, at load, over the variants its clause's text shows:
/// a family applied at a `FOR ALL` name shows its variants whatever the name is bound to.
#[test]
fn a_union_clause_is_checked_at_load_over_the_variants_its_text_shows() {
    let open = |arms: &str| {
        format!(
            "{OPTION}\nEXPR FOR ALL #[Elt] #(OPEN x :(Elt -> Option)) -> Any = #(\n  \
             MATCH x UNDER :(Elt -> Option) WITH {arms})"
        )
    };
    let missing = refused(&open("#{Some: (it)}"));
    assert!(
        missing.contains("has no arm for :(None {Elem = Elt})"),
        "{missing}"
    );
    let ran = run(&format!(
        "{}\nPRINT (OPEN (Option.Some 1))\nPRINT (OPEN (Option.Some \"soon\"))\n\
         PRINT (OPEN (Option.None null))",
        open("#{Some: (it), None: (0)}")
    ));
    assert_eq!(ran, "1\nsoon\n0");
}

/// A type declared in a body over a `FOR ALL` name shows what its declaration writes, at every
/// binding: its variants, a newtype's fields and a family's parameters are licensed by the text,
/// and a member or an arm it does not show is refused at load.
#[test]
fn a_type_declared_over_a_for_all_name_licenses_what_its_declaration_shows() {
    let subject = |body: &str| {
        format!("EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  {body}\n)\nPRINT (SUBJECT 5)")
    };
    let chain = "UNION Chain = #{Link: :{v :Elt}, End: Null}";
    assert_eq!(
        run(&subject(&format!(
            "{chain}\n  LET c = (Chain.Link {{v = x}})\n  \
             MATCH c UNDER Chain WITH #{{Link: (it.v), End: (0)}}"
        ))),
        "5"
    );
    assert_eq!(
        run(&subject(
            "NEWTYPE Box = :{v :Elt}\n  LET n :(Box.v) = x\n  n"
        )),
        "5"
    );
    assert_eq!(
        run(&subject(
            "UNION (Item -> Tree) = #{Leaf: Item, Pair: :{l :Elt}}\n  \
             LET t :(Number -> Tree) = (Tree.Leaf 1)\n  t"
        )),
        ":(Leaf {Item = Number})(1)"
    );
    let bogus = refused(&subject(&format!(
        "{chain}\n  LET c :(Chain.Bogus) = 1\n  c"
    )));
    assert!(bogus.contains("has no member Bogus"), "{bogus}");
    let uncovered = refused(&subject(&format!(
        "{chain}\n  MATCH (Chain.End null) UNDER Chain WITH #{{Link: (1)}}"
    )));
    assert!(uncovered.contains("has no arm for"), "{uncovered}");
}

/// A body reads a field only some bindings have through a `MATCH … WITH` guard naming the record
/// type.
#[test]
fn a_guard_reads_a_field_only_some_bindings_have() {
    let source = "EXPR FOR ALL #[Elt] #(SIZEOR x :Elt) -> Any = #(\n  \
                  MATCH x WITH #{:(:{size :Any}): (it.size), _: (0)})\n\
                  PRINT (SIZEOR {size = 5, width = 2})\n\
                  PRINT (SIZEOR 7)";
    assert_eq!(run(source), "5\n0");
}

/// A field read of a value through a `FOR ALL` name is refused at load where the name's bound
/// declares no such field or member, and reads each binding's field where it does.
#[test]
fn a_field_read_through_a_for_all_name_is_licensed_by_its_bound() {
    let size =
        |bound: &str| format!("EXPR FOR ALL #{{Elt: {bound}}} #(SIZE x :Elt) -> Any = #(x.size)");
    let refusal = refused(&size("Any"));
    assert!(
        refusal.contains("Elt has no field size: its bound Any declares none"),
        "{refusal}"
    );
    assert_eq!(
        run(&format!(
            "{}\nPRINT (SIZE {{size = 3}})",
            size(":{size :Any}")
        )),
        "3"
    );
    let member = "SIG Ord FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
                  MODULE ints = (LET zero = 0)\n\
                  EXPR FOR ALL #{Elt: Ord} #(ZERO m :Elt) -> Any = #(m.zero)\n\
                  PRINT (ZERO ints)";
    assert_eq!(run(member), "0", "a signature bound licenses its members");
    let undeclared = refused(&member.replace("(m.zero)", "(m.one)"));
    assert!(
        undeclared.contains("Elt has no field one: its bound Ord declares none"),
        "{undeclared}"
    );
}

/// A member read through a union holding a `FOR ALL` name is licensed where every member's bound
/// declares it, and read as each binding's module declares it, joined over the union.
#[test]
fn a_member_read_through_a_union_holding_a_for_all_name_is_licensed_by_its_bounds() {
    let union = |bound: &str| {
        format!(
            "SIG Has = #[(VAL zero :Number)]\n\
             SIG Named = #[(VAL zero :Str)]\n\
             MODULE ints = (LET zero = 0)\n\
             MODULE words = (LET zero = \"none\")\n\
             EXPR FOR ALL #{{Elt: {bound}}} #(ZERO m :(Elt | Has)) -> Any = #(m.zero)\n\
             PRINT (ZERO ints)\n\
             PRINT (ZERO words)"
        )
    };
    assert_eq!(run(&union("Named")), "0\nnone");
    // A meet over the name declares what either of its operands does.
    let met = "SIG Shaped FOR ALL #[Shape] = #[(VAL make :Shape)]\n\
               SIG Fixed = #[(LET Shape = Number) (VAL size :Number)]\n\
               MODULE nums = ((LET make = 3) (LET Shape = Number) (LET size = 1))\n\
               EXPR FOR ALL #[Elt] #(SHAPE x :Elt AND m :Any) -> Any = #(\n  \
                 LET Both = :((Shaped WITH {Shape = Elt}) & Fixed)\n  \
                 LET take = (FN :{n :Both} -> Any = #(\n    \
                   LET s :(n.Shape) = n.make\n    \
                   s))\n  \
                 take {n = m})\n\
               PRINT (SHAPE 1 AND nums)";
    assert_eq!(run(met), "3");
    let refusal = refused(&union("Any"));
    assert!(
        refusal.contains("has no field zero: its bound Any declares none"),
        "{refusal}"
    );
}

/// A `FOR ALL` bound inside a type may not name the encloser's `Elt`: each binding would make
/// `Local` a different signature, and no type written over names holds a scheme whose bound a
/// binding decides. A `VAL`'s scheme and a `SIG`'s own group are refused alike, at the bound.
#[test]
fn a_bound_inside_a_type_over_an_encloser_name_is_refused_at_load() {
    let refusal = |declaration: &str| {
        refused(&format!(
            "EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  {declaration}\n  x\n)\n\
             PRINT (SUBJECT 5)"
        ))
    };
    let wanted = "a `FOR ALL` bound inside a type names a type bound where the program runs";
    for declaration in [
        "SIG Local = #[(VAL pick :(FN FOR ALL #{Item: Elt} :{y :Item} -> Item))]",
        "SIG Box FOR ALL #{Carrier: Elt} = #[(VAL zero :Carrier)]",
    ] {
        let refusal = refusal(declaration);
        assert!(refusal.contains(wanted), "{refusal}");
    }
}

/// A type name a `USING … SCOPE` surfaces is the type its module's signature declares it at, where
/// it declares it exactly — a manifest member, or a head parameter the signature's application
/// pins — and licenses what that type shows; a member the signature leaves abstract licenses
/// nothing.
#[test]
fn a_surfaced_type_name_is_what_its_module_s_signature_declares() {
    let palette = |variant: &str| {
        format!(
            "MODULE palette = (UNION Color = #{{Red: Null, Blue: Null}})\n\
             PRINT (USING palette SCOPE (({variant} null)))"
        )
    };
    assert_eq!(run(&palette("Color.Red")), "Red(null)");
    let green = refused(&palette("Color.Green"));
    assert!(green.contains("has no member Green"), "{green}");
    let shaped = |pinned: &str| {
        format!(
            "SIG Shaped FOR ALL #[Shape] = #[(VAL s :Shape)]\n{MAYBE}\n\
             MODULE m = (LET s = (Maybe.None null))\n\
             EXPR #(OPEN n :Any) -> Any = #(\n  \
             USING (n :! {pinned}) SCOPE (\n    \
             LET x :(Shape.None) = (Maybe.None null)\n    x))\n\
             PRINT (OPEN m)"
        )
    };
    assert_eq!(run(&shaped("(Shaped WITH {Shape = Maybe})")), "None(null)");
    let abstract_ = refused(&shaped("Shaped"));
    assert!(
        abstract_.contains("Shape has no member None: its bound Any declares none"),
        "{abstract_}"
    );
}
