# Surface reference

Every surface form on one page. For explanations and runnable examples, follow
the link in each section to the relevant chapter.

## Binding and output — see [3](03-names-and-dispatch.md)

| Form                          | Meaning                                            |
|-------------------------------|----------------------------------------------------|
| `LET <name> = <value>`        | Bind a value to a lowercase name. Evaluates to the value. |
| `LET <TypeName> = <type>`     | Bind a type to a type name (capitalized + lowercase). |
| `PRINT <value>`               | Print a value and a newline. Evaluates to the printed string. |

## Functions — see [4](04-functions.md)

| Form                                       | Meaning                                  |
|--------------------------------------------|------------------------------------------|
| `FN (<signature>) -> <Type> = (<body>)`    | Define a function — a keyword/slot shape with an enforced return type. |
| `FN :{<fields>} -> <Type> = (<body>)`      | Anonymous function: a keyword-less record-schema shape. |
| `<keyword> <args>`                         | Call a function by writing its shape (e.g. `ECHO 21`). |
| `<fn> {name = value, ...}`                 | Call a captured function by named arguments. |

## Data types — see [5](05-tagged-unions.md), [7](07-records.md), [8](08-newtypes.md)

| Form                                       | Meaning                                  |
|--------------------------------------------|------------------------------------------|
| `UNION <Name> = (<Tag> :<Type> ...)`       | Declare a tagged union (sum type).       |
| `(<Union> (<Tag> <value>))`                | Construct a tagged value.                |
| `:(<Union> <Tag>)`                         | A slot type admitting one variant.       |
| `NEWTYPE <Name> = <Type>`                  | Declare a nominal type over a representation. |
| `NEWTYPE <Name> = :{<field> :<Type>, ...}` | Declare a record type (named fields).    |
| `(<Type> {field = value, ...})`            | Construct a record value.                |
| `<record>.<field>`                         | Read a field off a `NEWTYPE` record.     |
| `(<fields>) FROM <record>`                 | Project a record's type to the named fields. |
| `RECURSIVE TYPES <Name> = ( <decls> )`     | Declare mutually recursive types together. |

## Control and errors — see [6](06-pattern-matching.md), [9](09-errors.md)

| Form                                              | Meaning                           |
|---------------------------------------------------|-----------------------------------|
| `MATCH (<value>) -> :<Type> WITH (<Tag> -> (<body>) ...)` | Branch on a union tag or boolean; `it` is the payload. |
| `TRY (<expr>) -> :<Type> WITH (<Tag> -> (<body>) ... )`   | Catch errors; arms are `Ok`, error-kind tags, and `_`. |
| `CATCH (<expr>)`                                   | Run an expression, returning a `Result` value. |
| `Result`, `Ok`, `Error`                           | Built-in result union and its variants. |

## Quoting — see [10](10-quoting.md)

| Form          | Meaning                                              |
|---------------|------------------------------------------------------|
| `#(<expr>)`   | Quote: capture an expression as a value, unevaluated. |
| `$(<expr>)`   | Evaluate a quoted-expression value in the current scope. |

## Modules — see [11](11-modules.md), [12](12-functors.md)

| Form                                             | Meaning                            |
|--------------------------------------------------|------------------------------------|
| `MODULE <Name> = (<bindings>)`                   | Group bindings under a name.       |
| `<Module>.<member>`                              | Read a module member.              |
| `SIG <Name> = (VAL <name> :<Type> ...)`          | Declare a signature (a module's type). |
| `VAL <name> :<Type>`                             | A required value member, inside a `SIG`. |
| `<Module> :! <Sig>`                              | Transparent ascription.            |
| `<Module> :\| <Sig>`                             | Opaque ascription.                 |
| `USING <Module> SCOPE (<body>)`                  | Run a body with a module's members in scope. |
| `FUNCTOR (<KW> <p> :<Sig>) -> <Type> = (<body>)` | A module parameterized by a module. |
| `<Sig> WITH {<Slot> = <Type>}`                   | Specialize a signature by pinning a type slot. |

## Type expressions — see [2](02-values-and-types.md)

| Form                          | Meaning                                            |
|-------------------------------|----------------------------------------------------|
| `Number` `Str` `Bool` `Null`  | Built-in scalar types.                             |
| `Any`                         | Accepts any value (opts a slot out of checking).   |
| `:(LIST OF <Type>)`           | List type.                                         |
| `:(MAP <Key> -> <Value>)`     | Map / dictionary type.                             |
| `:(FN (<params>) -> <Result>)`| Function type.                                     |
| `TYPE (Type AS Wrap)`         | A higher-kinded type member, inside a `SIG`.       |

Token rule throughout: **keywords** are ≥2 uppercase letters with no lowercase;
**type names** are uppercase-leading with ≥1 lowercase letter; **identifiers**
are lowercase-leading. A lone capital like `T` is neither, and a parse error.
