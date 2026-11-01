# Comments

A line comment, so a program can explain itself.

**Problem.** Koan has no comment syntax: every character of a program is code
or a string, so an explanation lives outside the program or in a string a
statement discards. [`sexlex`](../../sexlex/README.md) reads every character as
whitespace, a bracket, a quote, a comma or part of a token.

**Acceptance criteria.**

- `;` outside a string starts a comment that runs to the end of its line, and
  the layout tree holds nothing of it.
- A line holding only a comment is no layout line, so it neither opens nor
  closes an indented block.
- A `;` inside a string is text, a template's text included.

**Directions.**

- *Where the rule lives — decided.* In `sexlex`: a comment may hold an unmatched
  quote or bracket, so only the reader can skip it.
- *A `;` inside a template's brace — open.* The brace is read as code, so a `;`
  there would comment out the rest of its line, the closing brace included:
  refuse a `;` inside a brace, or end the comment at the brace.

## Dependencies

**Requires:** none — foundation.

**Unblocks:** none — a leaf.
