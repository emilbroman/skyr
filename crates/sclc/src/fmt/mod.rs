mod block;
mod build;
mod render;

use crate::{Lexer, Span, Token};

use crate::ast::FileMod;

#[derive(Debug, Clone)]
pub(crate) struct Comment {
    pub span: Span,
    pub text: String,
}

fn collect_comments(source: &str) -> Vec<Comment> {
    Lexer::new(source)
        .filter_map(|tok| match *tok.as_ref() {
            Token::Comment(text) => Some(Comment {
                span: tok.span(),
                text: text.to_owned(),
            }),
            _ => None,
        })
        .collect()
}

pub struct Formatter;

impl Formatter {
    pub fn format(source: &str, file_mod: &FileMod) -> String {
        let comments = collect_comments(source);
        let mut builder = build::BlockBuilder::new(comments);
        let block = builder.build_file_mod(file_mod);
        let mut renderer = render::Renderer::new(100, 4);
        renderer.render(&block);
        let output = renderer.into_output();
        // Ensure file ends with a single newline
        let trimmed = output.trim_end().to_owned();
        if trimmed.is_empty() {
            trimmed
        } else {
            trimmed + "\n"
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{ModuleId, parse_file_mod};

    use super::Formatter;

    fn format(source: &str) -> String {
        let module_id = ModuleId::new(vec!["Test".to_owned()]);
        let diagnosed = parse_file_mod(source, &module_id);
        assert!(
            !diagnosed.diags().has_errors(),
            "parse errors: {:?}",
            diagnosed
                .diags()
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
        let file_mod = diagnosed.into_inner();
        Formatter::format(source, &file_mod)
    }

    #[test]
    fn formats_let_binding() {
        let result = format("let x = 42");
        assert_eq!(result, "let x = 42\n");
    }

    #[test]
    fn formats_export_let() {
        let result = format("export let x = 42");
        assert_eq!(result, "export let x = 42\n");
    }

    #[test]
    fn formats_import() {
        let result = format("import Std/List");
        assert_eq!(result, "import Std/List\n");
    }

    #[test]
    fn formats_fn_expr() {
        // Short fn body stays inline
        let result = format("let f = fn(x: Int)\n\tx + 1");
        assert_eq!(result, "let f = fn(x: Int) x + 1\n");
    }

    #[test]
    fn formats_if_else_simple() {
        // Short if/else stays on one line
        let result = format("let x = if (a) b else c");
        assert_eq!(result, "let x = if (a) b else c\n");
    }

    #[test]
    fn formats_if_else_multiline_then() {
        // Record fits inline, so the entire if/else fits on one line
        let result = format("let x = if (a) {v: 1} else c");
        assert_eq!(result, "let x = if (a) { v: 1 } else c\n");
    }

    #[test]
    fn formats_if_else_multiline_else() {
        let result = format("let x = if (a) b else {v: 1}");
        assert_eq!(result, "let x = if (a) b else { v: 1 }\n");
    }

    #[test]
    fn formats_record_shorthand() {
        // Short record stays inline
        let result = format("let r = { a: a, b: 1 }");
        assert_eq!(result, "let r = { a, b: 1 }\n");
    }

    #[test]
    fn formats_empty_record() {
        let result = format("let r = {}");
        assert_eq!(result, "let r = {}\n");
    }

    #[test]
    fn formats_type_def() {
        // Short record type stays inline
        let result = format("type Foo { bar: Int, baz: Str }");
        assert_eq!(result, "type Foo { bar: Int, baz: Str }\n");
    }

    #[test]
    fn formats_binary_ops() {
        let result = format("let x = 1 + 2 * 3");
        assert_eq!(result, "let x = 1 + 2 * 3\n");
    }

    #[test]
    fn formats_string_interpolation() {
        let result = format(r#"let x = "hello {name}!""#);
        assert_eq!(result, "let x = \"hello {name}!\"\n");
    }

    #[test]
    fn formats_list_comprehension() {
        let result = format("let xs = [for (x in list) x]");
        assert_eq!(result, "let xs = [for (x in list) x]\n");
    }

    #[test]
    fn preserves_comments() {
        let result = format("// top comment\nlet x = 42");
        assert_eq!(result, "// top comment\nlet x = 42\n");
    }

    #[test]
    fn preserves_trailing_comments() {
        // Trailing comments force multiline because the item has comments
        let result = format("type Foo {\n\tbar: Int, // a comment\n\tbaz: Str,\n}");
        assert_eq!(
            result,
            "type Foo {\n\tbar: Int, // a comment\n\tbaz: Str,\n}\n"
        );
    }

    #[test]
    fn formats_exception() {
        let result = format("let E = exception");
        assert_eq!(result, "let E = exception\n");
    }

    #[test]
    fn formats_exception_with_type() {
        let result = format("let E = exception(Str)");
        assert_eq!(result, "let E = exception(Str)\n");
    }

    #[test]
    fn formats_try_catch() {
        let result = format("let x = try f()\ncatch E(e): e");
        assert_eq!(result, "let x = try f()\ncatch E(e): e\n");
    }

    #[test]
    fn formats_dict() {
        // Short dict stays inline
        let result = format(r#"let d = #{"a": 1}"#);
        assert_eq!(result, "let d = #{ \"a\": 1 }\n");
    }

    #[test]
    fn formats_optional_type() {
        let result = format("type Foo { x: Int? }");
        assert_eq!(result, "type Foo { x: Int? }\n");
    }

    #[test]
    fn formats_nil() {
        let result = format("let x = nil");
        assert_eq!(result, "let x = nil\n");
    }

    #[test]
    fn formats_property_access() {
        let result = format("let x = a.b.c");
        assert_eq!(result, "let x = a.b.c\n");
    }

    #[test]
    fn formats_generic_call() {
        let result = format("let x = f<Int>(1)");
        assert_eq!(result, "let x = f<Int>(1)\n");
    }

    #[test]
    fn formats_extern() {
        let result = format(r#"let f = extern "Foo.bar": fn(Int) Str"#);
        assert_eq!(result, "let f = extern \"Foo.bar\": fn(Int) Str\n");
    }

    #[test]
    fn formats_blank_line_between_stmts() {
        let result = format("let x = 1\nlet y = 2");
        assert_eq!(result, "let x = 1\n\nlet y = 2\n");
    }

    #[test]
    fn formats_unary_negate() {
        let result = format("let x = -1");
        assert_eq!(result, "let x = -1\n");
    }

    #[test]
    fn formats_let_in_expr() {
        let result = format("let x = let y = 1; y + 1");
        assert_eq!(result, "let x = let y = 1;\ny + 1\n");
    }

    #[test]
    fn formats_bool_literals() {
        let result = format("let x = true");
        assert_eq!(result, "let x = true\n");
    }

    #[test]
    fn formats_raise() {
        let result = format("let x = raise E");
        assert_eq!(result, "let x = raise E\n");
    }

    #[test]
    fn formats_empty_list() {
        let result = format("let x = []");
        assert_eq!(result, "let x = []\n");
    }

    #[test]
    fn formats_empty_dict() {
        let result = format("let x = #{}");
        assert_eq!(result, "let x = #{}\n");
    }

    #[test]
    fn formats_fn_type() {
        let result = format("type F fn(Int, Str) Bool");
        assert_eq!(result, "type F fn(Int, Str) Bool\n");
    }

    #[test]
    fn formats_dict_type() {
        let result = format("type D #{Str: Int}");
        assert_eq!(result, "type D #{Str: Int}\n");
    }

    #[test]
    fn formats_list_type() {
        let result = format("type L [Int]");
        assert_eq!(result, "type L [Int]\n");
    }

    #[test]
    fn formats_generic_type_def() {
        // Short record stays inline
        let result = format("type Pair<A, B> { first: A, second: B }");
        assert_eq!(result, "type Pair<A, B> { first: A, second: B }\n");
    }

    #[test]
    fn formats_type_application() {
        let result = format("type IntList List<Int>");
        assert_eq!(result, "type IntList List<Int>\n");
    }

    #[test]
    fn formats_bounded_type_param() {
        // Short enough to fit on one line
        let result = format("let f = fn<T <: { name: Str }>(x: T)\n\tx.name");
        assert_eq!(result, "let f = fn<T <: { name: Str }>(x: T) x.name\n");
    }

    #[test]
    fn formats_float() {
        let result = format("let x = 3.14");
        assert_eq!(result, "let x = 3.14\n");
    }

    #[test]
    fn formats_consecutive_imports_single_newline() {
        let result = format("import Std/List\nimport Std/Map");
        assert_eq!(result, "import Std/List\nimport Std/Map\n");
    }

    #[test]
    fn formats_import_then_let_with_blank_line() {
        let result = format("import Std/List\nlet x = 1");
        assert_eq!(result, "import Std/List\n\nlet x = 1\n");
    }

    #[test]
    fn formats_list_with_multiline_items() {
        // Records fit inline, so the entire list fits on one line
        let result = format("let x = [{a: 1}, {b: 2}]");
        assert_eq!(result, "let x = [{ a: 1 }, { b: 2 }]\n");
    }

    #[test]
    fn formats_list_with_simple_items_inline() {
        let result = format("let x = [1, 2, 3]");
        assert_eq!(result, "let x = [1, 2, 3]\n");
    }

    #[test]
    fn formats_for_with_multiline_body() {
        // Record fits inline, so for-in stays on one line
        let result = format("let x = [for (item in list) {name: item}]");
        assert_eq!(result, "let x = [for (item in list) { name: item }]\n");
    }

    #[test]
    fn formats_for_with_simple_body_inline() {
        let result = format("let x = [for (item in list) item]");
        assert_eq!(result, "let x = [for (item in list) item]\n");
    }

    // ── Width-aware wrapping tests ───────────────────────────────

    #[test]
    fn wraps_long_record_to_multiline() {
        let result = format(
            "let r = { veryLongFieldNameAlphaValue: someValueAlphaResult, veryLongFieldNameBravoValue: someValueBravoResult }",
        );
        assert_eq!(
            result,
            "let r = {\n\tveryLongFieldNameAlphaValue: someValueAlphaResult,\n\tveryLongFieldNameBravoValue: someValueBravoResult,\n}\n"
        );
    }

    #[test]
    fn wraps_long_if_else() {
        let result = format(
            "let x = if (someLongConditionVariable) someReallyLongThenExpressionValue else someReallyLongElseExpressionValue",
        );
        // Branches unfold to separate lines
        assert_eq!(
            result,
            "let x = if (someLongConditionVariable) someReallyLongThenExpressionValue\nelse someReallyLongElseExpressionValue\n"
        );
    }

    #[test]
    fn wraps_fn_with_long_body() {
        let result = format(
            "let f = fn(x: Int) someReallyLongFunctionCall(withManyArguments, andMoreArguments, andEvenMoreArguments)",
        );
        assert_eq!(
            result,
            "let f = fn(x: Int)\n\tsomeReallyLongFunctionCall(withManyArguments, andMoreArguments, andEvenMoreArguments)\n"
        );
    }

    #[test]
    fn formats_if_else_chain() {
        let result = format("let x = if (a) b else if (c) d else e");
        assert_eq!(result, "let x = if (a) b else if (c) d else e\n");
    }

    // ── User-controlled unfolding ────────────────────────────────

    #[test]
    fn respects_user_multiline_record() {
        // Opening brace on a separate line from first field → stays multiline
        let result = format("let r = {\n\ta: 1,\n}");
        assert_eq!(result, "let r = {\n\ta: 1,\n}\n");
    }

    #[test]
    fn folds_same_line_record() {
        // Opening brace on same line as first field → folds when it fits
        let result = format("let r = { a: 1 }");
        assert_eq!(result, "let r = { a: 1 }\n");
    }

    #[test]
    fn respects_user_multiline_dict() {
        let result = format("let d = #{\n\t\"a\": 1,\n}");
        assert_eq!(result, "let d = #{\n\t\"a\": 1,\n}\n");
    }

    #[test]
    fn respects_user_multiline_record_type() {
        let result = format("type Foo {\n\tx: Int,\n}");
        assert_eq!(result, "type Foo {\n\tx: Int,\n}\n");
    }

    // ── Hugging for nested delimiters ────────────────────────────

    #[test]
    fn hugs_record_in_call() {
        // Single record arg → hugging layout avoids double-indentation
        let result = format("let x = f({\n\ta: 1,\n\tb: 2,\n})");
        assert_eq!(result, "let x = f({\n\ta: 1,\n\tb: 2,\n})\n");
    }

    #[test]
    fn hugs_list_in_call() {
        // List too wide to fold → hugging layout (101 chars if folded, exceeds max_width=100)
        let result = format(
            "let x = ff([longValueAlphaX, longValueBravo, longValueCharlie, longValueDelta, longValueEchoFoxtrot])",
        );
        assert_eq!(
            result,
            "let x = ff([\n\tlongValueAlphaX,\n\tlongValueBravo,\n\tlongValueCharlie,\n\tlongValueDelta,\n\tlongValueEchoFoxtrot,\n])\n"
        );
    }

    #[test]
    fn hugs_record_in_list() {
        let result = format("let x = [{\n\ta: 1,\n}]");
        assert_eq!(result, "let x = [{\n\ta: 1,\n}]\n");
    }

    #[test]
    fn no_hug_when_multiple_args() {
        // Multiple args → normal unfolded, no hugging
        let result = format("let x = f({\n\ta: 1,\n}, {\n\tb: 2,\n})");
        assert_eq!(
            result,
            "let x = f(\n\t{\n\t\ta: 1,\n\t},\n\t{\n\t\tb: 2,\n\t},\n)\n"
        );
    }

    #[test]
    fn inline_record_in_call_when_fits() {
        // Same-line short record in a call → folds entirely inline
        let result = format("let x = f({ a: 1 })");
        assert_eq!(result, "let x = f({ a: 1 })\n");
    }
}
