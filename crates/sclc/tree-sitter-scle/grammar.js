/// <reference types="tree-sitter-cli/dsl" />

module.exports = grammar(require("../tree-sitter-scl/grammar"), {
    name: "scle",

    rules: {
        source_file: ($) =>
            seq(
                repeat($.import_statement),
                field("type", $._type_expression),
                field("body", $._expression),
            ),
    },
});
