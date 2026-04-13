# tree-sitter-scle

Tree-sitter grammar for **SCLE** (SCL Expression), a value format based on the SCL language. An SCLE file (`.scle`) contains an optional sequence of imports, followed by a single type expression, followed by a single body expression. This grammar inherits all rules from [tree-sitter-scl](../tree-sitter-scl/) and overrides only the root `source_file` rule.
