# Skyr LSP Server

The LSP crate implements a Language Server Protocol server for SCL, providing editor features like diagnostics, hover, completion, and navigation.

## Role in the Architecture

The LSP server is started by the [CLI](../cli/) via `skyr lsp` and communicates with editors over JSON-RPC (stdin/stdout). It uses [SCLC](../sclc/) to compile and analyze SCL programs on the fly.

```
Editor ←→ JSON-RPC ←→ LSP Server → SCLC (compile + type-check)
```

## Components

| Component | Description |
|-----------|-------------|
| **LanguageServer** | Core server struct; dispatches incoming requests and notifications to handlers |
| **LspTransport** | JSON-RPC transport with LSP Content-Length framing over async readers/writers |
| **DocumentCache** | In-memory overlay of open editor files, layered on top of the filesystem |
| **OverlaySource** | `SourceRepo` implementation that checks the document cache before reading from disk |
| **Handlers** | Per-feature modules for lifecycle, hover, completion, and navigation |

## How It Works

1. The CLI creates a `LanguageServer` and an `LspTransport` over stdin/stdout.
2. On `initialize`, the server advertises its capabilities (full text sync, hover, completion, go-to-definition, references, document symbols).
3. When a document is opened or changed, the server updates its `DocumentCache` and re-runs `sclc::compile` to publish diagnostics.
4. For queries (hover, completion, go-to-definition, references), the server loads the full program with resolved imports and uses `sclc::Cursor` to gather context-aware information at the requested position.

### Document Overlay

The `OverlaySource` wraps a filesystem-based `SourceRepo` and intercepts `read_file` calls: if a file is open in the editor, its in-memory content is returned instead of the on-disk version. This ensures diagnostics and queries always reflect the latest unsaved edits.

## Supported LSP Features

| Feature | Method | Description |
|---------|--------|-------------|
| Diagnostics | `textDocument/publishDiagnostics` | Compiler errors and warnings pushed on open/change/save |
| Hover | `textDocument/hover` | Type information for the symbol under the cursor |
| Completion | `textDocument/completion` | Context-aware completions (variables, fields, imports) |
| Go to Definition | `textDocument/definition` | Jump to where a symbol is declared |
| Find References | `textDocument/references` | All usages of the symbol under the cursor |
| Document Symbols | `textDocument/documentSymbol` | Outline of let bindings, exports, types, and imports |

## Related Crates

- [SCLC](../sclc/) — compiler and type checker powering all analysis
- [CLI](../cli/) — hosts the `skyr lsp` command that starts the server
