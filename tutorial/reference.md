# Surface reference

Every surface form on one page. For explanations and runnable examples, follow
the link in each section to the relevant chapter.

## Binding and output — see [3](03-names-and-dispatch.md)

| Form                          | Meaning                                            |
|-------------------------------|----------------------------------------------------|
| `LET <name> = <value>`        | Bind a value, a type or code to a lowercase name. Evaluates to what it binds. |
| `LET <name> :<Type> = <value>` | Bind a value at a stated type: checked against it and viewed at it, as `:!` does. |
| `LET <TypeName> = <type>`     | Bind a type to a type name (capitalized + lowercase). |
| `PRINT <value>`               | Print a value and a newline. Evaluates to the printed string. |

## Functions — see [4](04-functions.md)

| Form                                       | Meaning                                  |
|--------------------------------------------|------------------------------------------|
| `EXPR #(<head>) -> <Type> = #(<body>)`     | Define an expression shape — a keyword/slot run with an enforced return type, reached by dispatch. |
| `EXPR FOR ALL #[<names>] #(<head>) -> <Type> = #(<body>)` | The same, quantified: each name is solved per call from the types the arguments carry. |
| `LET <name> = FN EXPR #(<head>) -> <Type> = #(<body>)` | Both channels from one statement: the value name and the shape's bucket. |
| `FN :{<fields>} -> <Type> = #(<body>)`     | A lambda: keyword-less, reached by name, called with a record of named arguments. |
| `FN FOR ALL #[<names>] :{<fields>} -> <Type> = #(<body>)` | The same, quantified: each name is solved per call from the types the arguments carry, and the body reads it as a type. Bound by a plain `LET` only in a module; anywhere but the head of a call it is used at the one type it is wanted at there (a `LET`'s stated type, a `:!`, a slot, a declared return), which must settle every name. |
| `<keyword> <args>`                         | Call a function by writing its shape (e.g. `ECHO 21`). |
| `<fn> {name = value, ...}`                 | Call a captured function by named arguments. |
| `CLOSE OVER (<captures>) (<block>)`        | Run a block over a region of its own, copying the named values in; only the block's last expression escapes, and it holds copies rather than the enclosing call. |
| `CLOSE (<block>)`                          | The same, with the capture list inferred from the names the block uses; `$(...)` and `USING … SCOPE` inside the block are refused. |

## Data types — see [5](05-tagged-unions.md), [7](07-records.md), [8](08-newtypes.md)

| Form                                       | Meaning                                  |
|--------------------------------------------|------------------------------------------|
| `UNION <Name> = #{<Tag>: <Type>, ...}`     | Declare a tagged union (sum type).       |
| `<Union>.<Tag>`                            | Name one variant as a type value.        |
| `<Union>.<Tag> <value>`                    | Construct a tagged value.                |
| `:(<Union>.<Tag>)`                         | A slot type admitting one variant.       |
| `NEWTYPE <Name> = <Type>`                  | Declare a nominal type over a representation. |
| `NEWTYPE <Name> = :{<field> :<Type>, ...}` | Declare a record type (named fields).    |
| `(<Type> {field = value, ...})`            | Construct a record value.                |
| `<record>.<field>`                         | Read a field off any record value.       |
| `#[<fields>] FROM <record>`                | Project a record's type to the named fields. |

## Control and errors — see [6](06-pattern-matching.md), [9](09-errors.md)

| Form                                              | Meaning                           |
|---------------------------------------------------|-----------------------------------|
| `MATCH <value> UNDER <Union> WITH #{<Tag>: (<body>), ...}` | Branch on a union's variants; every variant needs an arm unless a `_` arm defaults the rest, and `it` is the payload. |
| `MATCH <value> WITH #{<Type or literal>: (<body>), ...}` | Branch on the value; each guard is a type or a scalar literal, the most specific admitting one runs, and `it` is the value at the guard's type. |
| `TRY (<expr>) -> :<Type> WITH #{<Tag>: (<body>), ...}`    | Catch errors; arms are `Ok`, error-kind names, and `_`. Uncaught kinds re-raise. |
| `CATCH (<expr>)`                                   | Run an expression, returning a `Result` value. |
| `Result.Ok <value>`, `Result.Error <value>`       | Built-in result union's variants, reached by projection. |

## Quoting — see [10](10-quoting.md)

| Form          | Meaning                                              |
|---------------|------------------------------------------------------|
| `#(<expr>)`   | Quote: capture an expression as a value, unevaluated. |
| `#[<element> ...]`, `#{<key>: <value>, ...}` | A list or dict literal with every element quoted; a `_` key stays bare. |
| `EVAL <code> -> <Type>` | Run a quote's code, its value checked against the declared return as a function's is (`-> Any` declares nothing). Its names bind only as the quote says, never in the scope `EVAL` is written in. |
| `x` inside a quote | A hole: binds only to a builtin, a binder in the quote's own code, or a name `USING` supplies. |
| `$x`, `$(<keyworded use>)` inside a quote | Resolve the name, or the one keyworded use, where the quote is written. |
| `\x`, `\(<keyworded use>)` inside a quote | Resolve where the code is run, from what the code parameter's type offers. |
| `<code> USING <record>` | Fill the holes the record's fields (or a module's members) name; the rest stay holes. A filled hole keeps its fill wherever the code goes. |
| `$..<name>` inside a quote | Splice the code elements of the list `<name>`, read where the quote is written: among statements each adds its statements, beside other parts each is one part. |
| `..$<name>` in a list literal inside a quote | Spread the list's value when the code runs. |
| `<code> <index>`, `<code> {from = <i>, to = <j>}` | Read a part of code, or the code of a run of its parts, as a list is read: `Result.Ok` of unbuilt code. |
| `BUILD <code>` | Give spliced or sliced code its exact type; built code is returned unchanged. |
| `<name> :Expression`  | A parameter taking one statement of code as a value; call sites pass `#(…)`. |
| `<name> :(Expression NEEDING #[<name> ...])` | A code parameter offering names; an `EVAL` of it supplies each where the `EVAL` is written. |
| `Block` `Expression` `Declaration` `Binder` `Literal` `Symbol` `Name` `Keyword` | The kinds of code under `Code`, each under the one before it in [the tree](10-quoting.md#what-kind-of-code-a-quote-is). |
| `:(UNBUILT <kind>)` | Code of that kind, needing whatever it needs: the type of spliced and sliced code before `BUILD`. |

## Modules — see [11](11-modules.md), [12](12-functors.md)

| Form                                             | Meaning                            |
|--------------------------------------------------|------------------------------------|
| `MODULE <name> = (<bindings>)`                   | Group bindings under a name (snake_case — a module is a value). |
| `MODULE <name> OVER #[<names>] = (<bindings>)`  | A module listing what it reads from the function around it; a keyworded definition is listed by its key, `(SCALE _)`. |
| `<module>.<member>`                              | Read a module member.              |
| `:(<module>.<Type>)`                             | A module's type member.            |
| `SIG <Name> = #[(VAL <name> :<Type>) ...]`       | Declare a signature (a module's type; Type-token name). |
| `SIG <Name> FOR ALL #[<names>] = #[...]`         | A signature over head parameters, each one type per module, which its members name. |
| `VAL <name> :<Type>`                             | A required value member, inside a `SIG`; its type may be a quantified function type. |
| `EXPR #(<head>) -> <Type>`                       | A required keyworded member, inside a `SIG` (bodyless; `FOR ALL` twin included, for a type each use works out afresh). |
| `<module> :! <Sig>`                              | Transparent ascription: the module seen at the signature, each head parameter visible. |
| `<module> :\| <Sig>`                             | Opaque ascription.                 |
| `USING <module> SCOPE (<body>)`                  | Run a body with a module's members in scope. |
| `EXPR #(<KW> <p> :<Sig>) -> Module = #(<body>)`    | A functor: a function returning a module (a module parameterized by a module). |
| `<Sig> WITH {<Param> = <Type>}`                  | An application: pin some of a signature's head parameters. |
| `:(<Sig> & <Sig>)`                               | The meet of two signatures: a module fits it when it fits both. |

## Reading collections — see [2](02-values-and-types.md#reading-lists-dictionaries-and-strings)

| Form                          | Meaning                                            |
|-------------------------------|----------------------------------------------------|
| `<list> <index>`, `<string> <index>` | Read the element or code point at a zero-based index: `Result.Ok` of it, or `Result.Error` of the index. |
| `<dict> <key>`                | Look a key up: `Result.Ok` of its value, or `Result.Error` of the key. |
| `<list> {from = <i>, to = <j>}` | A slice of a list or a string, `from` up to but not including `to`; either bound may be left out. Backward when `to` is below `from` or is `-1`. `Result.Error` of the first bound out of range. |
| `LENGTH <value>`              | A list's elements, a dict's entries, a string's code points or code's parts, counted. |
| `[..<list> <element> ...]`    | Spread a list's elements into a list literal; `..(<expr>)` spreads a parenthesized expression's. |

## Building strings — see [2](02-values-and-types.md#building-strings)

| Form                          | Meaning                                            |
|-------------------------------|----------------------------------------------------|
| `%"… {<expr>} …"`, `%'…'`     | A template: a `Str` of its text, each brace's expression rendered into it as `PRINT` renders it. `{{` and `}}` write a brace. |
| `%"… %{<expr>} …"`            | A brace keeping its value apart: the template is `{strings :(LIST OF Str), values :(LIST OF …)}`, the kept values and the n + 1 strings around them. |
| `JOIN <list>`                 | A list's elements, each rendered as `PRINT` renders it, written one after another as a `Str`. |
| `JOIN <list> BY <string>`     | The same, with the separator between consecutive elements. |

## Type expressions — see [2](02-values-and-types.md)

| Form                          | Meaning                                            |
|-------------------------------|----------------------------------------------------|
| `Number` `Str` `Bool` `Null`  | Built-in scalar types.                             |
| `Value`                       | Accepts any ordinary value.                        |
| `Type`                        | Accepts any type.                                  |
| `Code`                        | Accepts any quoted code.                           |
| `Any`                         | Accepts any value, type or code (opts a slot out of checking). |
| `:(LIST OF <Type>)`           | List type.                                         |
| `:(MAP <Key> -> <Value>)`     | Map / dictionary type.                             |
| `:(FN :{<params>} -> <Result>)`| Lambda type — the parameter list is a record type (`:{}` when nullary). |
| `:(FN FOR ALL #[<names>] :{<params>} -> <Result>)` | The quantified lambda type, written only as a `VAL` member's type inside a `SIG`. |
| `:(EXPR #(<head>) -> <Result>)` | Expression-shape type — the keyword/slot run a keyworded definition registers for dispatch; write `_` at each slot. |
| `:(EXPR FOR ALL #[<names>] #(<head>) -> <Result>)` | The quantified shape type, written only inside a `SIG`. |
| `FOR ALL #{<Name>: <Type>, ...}` | Bounded type parameters, each name to its bound, on a definition or a `SIG`'s head; a name in a `#[…]` group is bounded by `Any`. See [12](12-functors.md#bounding-a-type-parameter). |
| `:(<Type> & <Type>)`          | Meet — a value of both types; mixes with `\|` only through parentheses. |
| `<value> :! <Type>`           | Ascription: check a value against a type and view it at that type. |
| `NEWTYPE (Key Val AS Pair)`   | A type constructor with one or more parameters.    |
| `UNION (Elem AS Option) = #{<Tag>: <Type>, ...}` | A union over type parameters; each variant is a constructor over all of them. See [12](12-functors.md#unions-over-type-parameters-union-elem-as-option). |
| `:(Pair {Key = Number, Val = Str})` | Apply a type constructor, binding each parameter by name. |
| `:(Number AS Wrap)`           | Shorthand for applying a one-parameter constructor.|

Token rule throughout: **keywords** are ≥2 uppercase letters with no lowercase;
**type names** are uppercase-leading with ≥1 lowercase letter; **identifiers**
are lowercase-leading. A lone capital like `T` is neither, and a parse error.
