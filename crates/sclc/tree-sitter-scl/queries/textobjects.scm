; Functions
(fn_expression) @function.around
(fn_expression
  body: (_) @function.inside)

; Types
(type_declaration) @class.around
(type_declaration
  type: (_) @class.inside)
(export_type_declaration) @class.around

; Function parameters
(fn_parameters) @parameter.around
(fn_parameter) @parameter.inside

; Call arguments
(call_expression
  "(" @parameter.around.start
  ")" @parameter.around.end)

; Comments
(comment) @comment.around
(comment) @comment.inside
