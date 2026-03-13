#include "tree_sitter/parser.h"

// Token types matching the `externals` array in grammar.js:
//   0: string_content
enum TokenType {
  STRING_CONTENT,
};

void *tree_sitter_scl_external_scanner_create(void) { return NULL; }

void tree_sitter_scl_external_scanner_destroy(void *payload) {}

unsigned tree_sitter_scl_external_scanner_serialize(void *payload,
                                                    char *buffer) {
  return 0;
}

void tree_sitter_scl_external_scanner_deserialize(void *payload,
                                                  const char *buffer,
                                                  unsigned length) {}

static void advance(TSLexer *lexer) { lexer->advance(lexer, false); }

bool tree_sitter_scl_external_scanner_scan(void *payload, TSLexer *lexer,
                                           const bool *valid_symbols) {
  if (!valid_symbols[STRING_CONTENT]) {
    return false;
  }

  // Scan string content: text between delimiters inside a string.
  // Stops before `{` (interpolation start), `"` (string end), or EOF.
  // Handles escape sequences: \n, \r, \t, \\, \{
  bool has_content = false;

  while (lexer->lookahead != 0) {
    if (lexer->lookahead == '\\') {
      // Escape sequence — consume backslash and the next character
      has_content = true;
      advance(lexer);
      if (lexer->lookahead != 0) {
        advance(lexer);
      }
    } else if (lexer->lookahead == '{' || lexer->lookahead == '"') {
      // Stop before interpolation start or string end
      break;
    } else {
      has_content = true;
      advance(lexer);
    }
  }

  if (has_content) {
    lexer->result_symbol = STRING_CONTENT;
    return true;
  }

  return false;
}
