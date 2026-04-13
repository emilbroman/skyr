; Indent after opening brackets
[
  (record)
  (record_type)
  (dict)
  (dict_type)
  (list)
  (fn_expression)
  (call_expression)
  (if_expression)
  (try_expression)
  (parenthesized_expression)
] @indent

; Dedent at closing brackets
[
  "}"
  "]"
  ")"
] @outdent
