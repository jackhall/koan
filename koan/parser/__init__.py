"""
- start with code
- remove string literals
- collapse significant whitespace using parentheses
- parse code into nested lists
- tokenize non-quoted strings, add string literals back in
"""

from .parens import nest_parens, ParseError
