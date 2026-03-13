/// <reference types="tree-sitter-cli/dsl" />

const PREC = {
  OR: 1,
  AND: 2,
  EQUALITY: 3,
  COMPARISON: 4,
  ADDITIVE: 5,
  MULTIPLICATIVE: 6,
  UNARY: 7,
  POSTFIX: 8,
};

module.exports = grammar({
  name: "scl",

  extras: ($) => [/\s/, $.comment],

  word: ($) => $.identifier,

  externals: ($) => [$.string_content],

  conflicts: ($) => [
    [$.binary_expression, $.unary_expression, $.call_expression],
    [$.binary_expression, $.call_expression],
    [$._atom_expression, $._type_expression_base],
    [$.record, $.record_type],
    [$.if_expression, $._list_item],
  ],

  rules: {
    // ── Top-level ──────────────────────────────────────────────

    source_file: ($) => repeat($._mod_stmt),

    _mod_stmt: ($) =>
      choice(
        $.import_statement,
        $.export_type_declaration,
        $.export_statement,
        $.type_declaration,
        $._expression,
        $.let_binding,
      ),

    // ── Statements ─────────────────────────────────────────────

    import_statement: ($) =>
      seq("import", $.import_path),

    import_path: ($) =>
      seq($.identifier, repeat(seq("/", $.identifier))),

    let_binding: ($) =>
      seq("let", field("name", $.identifier), "=", field("value", $._expression)),

    export_statement: ($) => seq("export", $.let_binding),

    type_declaration: ($) =>
      seq(
        "type",
        field("name", $.identifier),
        optional($.type_parameters),
        field("type", $._type_expression),
      ),

    export_type_declaration: ($) => seq("export", $.type_declaration),

    // ── Expressions ────────────────────────────────────────────

    _expression: ($) =>
      choice(
        $.if_expression,
        $.let_expression,
        $.fn_expression,
        $.extern_expression,
        $.raise_expression,
        $.try_expression,
        $.binary_expression,
        $.unary_expression,
        $.call_expression,
        $.property_access,
        $._atom_expression,
      ),

    binary_expression: ($) =>
      choice(
        prec.left(PREC.OR, seq(field("left", $._expression), field("operator", "||"), field("right", $._expression))),
        prec.left(PREC.AND, seq(field("left", $._expression), field("operator", "&&"), field("right", $._expression))),
        prec.left(PREC.EQUALITY, seq(field("left", $._expression), field("operator", "=="), field("right", $._expression))),
        prec.left(PREC.EQUALITY, seq(field("left", $._expression), field("operator", "!="), field("right", $._expression))),
        prec.left(PREC.COMPARISON, seq(field("left", $._expression), field("operator", "<"), field("right", $._expression))),
        prec.left(PREC.COMPARISON, seq(field("left", $._expression), field("operator", "<="), field("right", $._expression))),
        prec.left(PREC.COMPARISON, seq(field("left", $._expression), field("operator", ">"), field("right", $._expression))),
        prec.left(PREC.COMPARISON, seq(field("left", $._expression), field("operator", ">="), field("right", $._expression))),
        prec.left(PREC.ADDITIVE, seq(field("left", $._expression), field("operator", "+"), field("right", $._expression))),
        prec.left(PREC.ADDITIVE, seq(field("left", $._expression), field("operator", "-"), field("right", $._expression))),
        prec.left(PREC.MULTIPLICATIVE, seq(field("left", $._expression), field("operator", "*"), field("right", $._expression))),
        prec.left(PREC.MULTIPLICATIVE, seq(field("left", $._expression), field("operator", "/"), field("right", $._expression))),
      ),

    unary_expression: ($) =>
      prec(PREC.UNARY, seq("-", field("operand", $._expression))),

    property_access: ($) =>
      prec.left(PREC.POSTFIX, seq(
        field("object", $._expression),
        ".",
        field("property", $.identifier),
      )),

    call_expression: ($) =>
      prec.left(PREC.POSTFIX, seq(
        field("function", $._expression),
        optional($.type_arguments),
        "(",
        optional(commaSep1($._expression)),
        ")",
      )),

    if_expression: ($) =>
      prec.right(seq(
        "if",
        "(",
        field("condition", $._expression),
        ")",
        field("consequence", $._expression),
        optional(seq("else", field("alternative", $._expression))),
      )),

    let_expression: ($) =>
      seq($.let_binding, ";", field("body", $._expression)),

    fn_expression: ($) =>
      prec.right(seq(
        "fn",
        optional($.type_parameters),
        "(",
        optional($.fn_parameters),
        ")",
        field("body", $._expression),
      )),

    fn_parameters: ($) => commaSep1($.fn_parameter),

    fn_parameter: ($) =>
      seq(field("name", $.identifier), ":", field("type", $._type_expression)),

    extern_expression: ($) =>
      seq("extern", field("name", $.string), ":", field("type", $._type_expression)),

    raise_expression: ($) =>
      prec.right(seq("raise", field("value", $._expression))),

    try_expression: ($) =>
      prec.right(seq(
        "try",
        field("body", $._expression),
        repeat1($.catch_clause),
      )),

    catch_clause: ($) =>
      choice(
        seq(
          "catch",
          field("exception", $.identifier),
          "(",
          field("binding", $.identifier),
          ")",
          ":",
          field("body", $._expression),
        ),
        seq(
          "catch",
          field("exception", $.identifier),
          ":",
          field("body", $._expression),
        ),
      ),

    exception_expression: ($) =>
      prec(1, choice(
        seq("exception", "(", $._type_expression, ")"),
        prec(-1, "exception"),
      )),

    // ── Atom expressions ───────────────────────────────────────

    _atom_expression: ($) =>
      choice(
        $.parenthesized_expression,
        $.string,
        $.dict,
        $.record,
        $.list,
        $.float,
        $.integer,
        $.boolean,
        $.nil,
        $.exception_expression,
        $.identifier,
      ),

    parenthesized_expression: ($) => seq("(", $._expression, ")"),

    // ── Collections ────────────────────────────────────────────

    record: ($) =>
      choice(
        seq("{", "}"),
        seq("{", commaSep1($.record_field), "}"),
      ),

    record_field: ($) =>
      choice(
        seq(field("name", $.identifier), ":", field("value", $._expression)),
        field("name", $.identifier),
      ),

    dict: ($) =>
      choice(
        seq("#", "{", "}"),
        seq("#", "{", commaSep1($.dict_entry), "}"),
      ),

    dict_entry: ($) =>
      seq(field("key", $._expression), ":", field("value", $._expression)),

    list: ($) =>
      choice(
        seq("[", "]"),
        seq("[", commaSep1($._list_item), "]"),
      ),

    _list_item: ($) =>
      choice(
        $.list_for_item,
        $.list_if_item,
        $._expression,
      ),

    list_for_item: ($) =>
      prec.dynamic(1, seq(
        "for",
        "(",
        field("variable", $.identifier),
        "in",
        field("iterable", $._expression),
        ")",
        field("body", $._list_item),
      )),

    list_if_item: ($) =>
      prec.dynamic(1, seq(
        "if",
        "(",
        field("condition", $._expression),
        ")",
        field("body", $._list_item),
      )),

    // ── Type expressions ───────────────────────────────────────

    _type_expression: ($) =>
      choice(
        $.optional_type,
        $._type_expression_base,
      ),

    optional_type: ($) =>
      prec(1, seq($._type_expression_base, "?")),

    _type_expression_base: ($) =>
      choice(
        $.fn_type,
        $.dict_type,
        $.record_type,
        $.list_type,
        $.type_property_access,
        alias($.identifier, $.type_identifier),
      ),

    list_type: ($) => seq("[", $._type_expression, "]"),

    fn_type: ($) =>
      prec.right(seq(
        "fn",
        optional($.type_parameters),
        "(",
        optional(commaSep1($._type_expression)),
        ")",
        field("return_type", $._type_expression),
      )),

    record_type: ($) =>
      choice(
        seq("{", "}"),
        seq("{", commaSep1($.record_type_field), "}"),
      ),

    record_type_field: ($) =>
      seq(field("name", $.identifier), ":", field("type", $._type_expression)),

    dict_type: ($) =>
      seq("#", "{", field("key", $._type_expression), ":", field("value", $._type_expression), "}"),

    type_property_access: ($) =>
      prec.left(2, seq($._type_expression_base, ".", alias($.identifier, $.type_identifier))),

    // ── Generics ───────────────────────────────────────────────

    type_parameters: ($) =>
      seq("<", commaSep1($.type_parameter), ">"),

    type_parameter: ($) =>
      choice(
        seq(field("name", $.identifier), "<:", field("bound", $._type_expression)),
        field("name", $.identifier),
      ),

    type_arguments: ($) =>
      seq("<", commaSep1($._type_expression), ">"),

    // ── Strings ────────────────────────────────────────────────

    string: ($) =>
      seq(
        '"',
        repeat(choice($.string_content, $.interpolation)),
        '"',
      ),

    interpolation: ($) =>
      seq("{", $._expression, "}"),

    // ── Literals ───────────────────────────────────────────────

    identifier: ($) => /[a-zA-Z_][a-zA-Z0-9_]*/,

    integer: ($) => token(choice("0", /[1-9][0-9]*/)),

    float: ($) =>
      token(seq(choice("0", /[1-9][0-9]*/), ".", /[0-9]+/)),

    boolean: ($) => choice("true", "false"),

    nil: ($) => "nil",

    comment: ($) => token(seq("//", /.*/)),
  },
});

/**
 * Comma-separated list with optional trailing comma.
 */
function commaSep1(rule) {
  return seq(rule, repeat(seq(",", rule)), optional(","));
}
