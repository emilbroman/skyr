; Scopes
(fn_expression) @local.scope
(let_expression) @local.scope
(list_for_item) @local.scope

; Definitions
(let_binding
  name: (identifier) @local.definition)
(fn_parameter
  name: (identifier) @local.definition)
(list_for_item
  variable: (identifier) @local.definition)
(catch_clause
  binding: (identifier) @local.definition)

; References
(identifier) @local.reference
