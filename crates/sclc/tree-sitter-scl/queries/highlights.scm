; Keywords
"import" @keyword
"let" @keyword
"fn" @keyword.function
"export" @keyword
"extern" @keyword
"if" @keyword.conditional
"else" @keyword.conditional
"for" @keyword.repeat
"in" @keyword
"type" @keyword
"exception" @keyword
"raise" @keyword
"try" @keyword
"catch" @keyword
"as" @keyword

; Literals
(integer) @number
(float) @number.float
(boolean) @constant.builtin
(nil) @constant.builtin

; Paths
(path_expression) @string.special.path

; Strings
(string) @string
(string_content) @string
(interpolation
  "{" @punctuation.special
  "}" @punctuation.special)

; Comments
(comment) @comment

; Operators
(binary_expression
  operator: _ @operator)
(unary_expression
  "-" @operator)

; Functions
(call_expression
  function: (identifier) @function.call)
(call_expression
  function: (property_access
    property: (identifier) @function.method.call))

; Type casts
(type_cast
  type: (type_identifier) @type)

; Types
(type_declaration
  name: (identifier) @type.definition)
(export_type_declaration
  (type_declaration
    name: (identifier) @type.definition))
(type_identifier) @type
(fn_parameter
  type: (type_identifier) @type)

; Properties
(property_access
  property: (identifier) @property)
(record_field
  name: (identifier) @property)
(record_type_field
  name: (identifier) @property)

; Variables
(let_binding
  name: (identifier) @variable)
(fn_parameter
  name: (identifier) @variable.parameter)
(list_for_item
  variable: (identifier) @variable)
(catch_clause
  binding: (identifier) @variable)
(catch_clause
  exception: (identifier) @type)

; Imports
(import_path
  (identifier) @module)

; Punctuation
"(" @punctuation.bracket
")" @punctuation.bracket
"[" @punctuation.bracket
"]" @punctuation.bracket
"{" @punctuation.bracket
"}" @punctuation.bracket
":" @punctuation.delimiter
"," @punctuation.delimiter
"." @punctuation.delimiter
";" @punctuation.delimiter
"=" @operator
"#" @punctuation.special
"<:" @operator
