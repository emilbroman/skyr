import * as monaco from "monaco-editor";
import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import type { CompletionItem, DiagnosticInfo, SclWorker } from "./client.js";

// Set up Monaco environment for web workers
self.MonacoEnvironment = {
    getWorker: () => new editorWorker(),
};

const LANGUAGE_ID = "scl";

let languageRegistered = false;

export function registerSclLanguage() {
    if (languageRegistered) return;
    languageRegistered = true;

    monaco.languages.register({ id: LANGUAGE_ID, extensions: [".scl"] });

    // Monarch tokenizer for basic syntax highlighting
    monaco.languages.setMonarchTokensProvider(LANGUAGE_ID, {
        keywords: [
            "let",
            "export",
            "import",
            "if",
            "then",
            "else",
            "match",
            "fn",
            "type",
            "as",
            "for",
            "in",
            "try",
            "catch",
            "raise",
            "extern",
            "exception",
        ],
        constants: ["true", "false", "nil"],
        operators: [
            "=",
            "==",
            "!=",
            ">",
            "<",
            ">=",
            "<=",
            "+",
            "-",
            "*",
            "/",
            "%",
            "&&",
            "||",
            "!",
            "|>",
            "=>",
            "->",
            "..",
            ".",
        ],
        tokenizer: {
            root: [
                // Comments
                [/\/\/.*$/, "comment"],
                // Paths (before strings and operators so `./ ` and `/` are matched correctly)
                [/\.{1,2}\/[\w.@-]+(?:\/[\w.@-]+)*/, "string"],
                [/\/[\w.@-]+(?:\/[\w.@-]+)*/, "string"],
                // Strings
                [/"/, "string", "@string"],
                // Numbers
                [/\b\d+(\.\d+)?\b/, "number"],
                // Uppercase identifiers (types/modules) — must be before lowercase
                [/\b[A-Z]\w*\b/, "type.identifier"],
                // Keywords, constants, and lowercase identifiers
                [
                    /\b[a-z_]\w*\b/,
                    {
                        cases: {
                            "@keywords": "keyword",
                            "@constants": "constant",
                            "@default": "identifier",
                        },
                    },
                ],
                // Operators
                [/[=><!~?:&|+\-*/^%]+/, "operator"],
                // Delimiters
                [/[{}()[\]]/, "@brackets"],
                [/[,;]/, "delimiter"],
            ],
            string: [
                [/[^"\\{]+/, "string"],
                [/\\./, "string.escape"],
                [/\{/, "string.interpolation", "@interpolation"],
                [/"/, "string", "@pop"],
            ],
            interpolation: [[/\}/, "string.interpolation", "@pop"], { include: "root" }],
        },
    });

    monaco.languages.setLanguageConfiguration(LANGUAGE_ID, {
        comments: { lineComment: "//" },
        brackets: [
            ["{", "}"],
            ["[", "]"],
            ["(", ")"],
        ],
        autoClosingPairs: [
            { open: "{", close: "}" },
            { open: "[", close: "]" },
            { open: "(", close: ")" },
            { open: '"', close: '"', notIn: ["string"] },
        ],
        surroundingPairs: [
            { open: "{", close: "}" },
            { open: "[", close: "]" },
            { open: "(", close: ")" },
            { open: '"', close: '"' },
        ],
    });
}

// ---------------------------------------------------------------------------
// Multi-model management
// ---------------------------------------------------------------------------

const models = new Map<string, monaco.editor.ITextModel>();

function fileUri(path: string): monaco.Uri {
    return monaco.Uri.parse(`file:///${path}`);
}

export function getOrCreateModel(path: string, content: string): monaco.editor.ITextModel {
    const existing = models.get(path);
    if (existing && !existing.isDisposed()) {
        return existing;
    }
    const model = monaco.editor.createModel(content, LANGUAGE_ID, fileUri(path));
    models.set(path, model);
    return model;
}

export function getModel(path: string): monaco.editor.ITextModel | undefined {
    const model = models.get(path);
    if (model && !model.isDisposed()) return model;
    models.delete(path);
    return undefined;
}

export function disposeModel(path: string) {
    const model = models.get(path);
    if (model) {
        model.dispose();
        models.delete(path);
    }
}

export function disposeAllModels() {
    for (const model of models.values()) {
        model.dispose();
    }
    models.clear();
}

export function renameModel(oldPath: string, newPath: string, content: string) {
    disposeModel(oldPath);
    return getOrCreateModel(newPath, content);
}

// ---------------------------------------------------------------------------
// Language providers (multi-file aware)
// ---------------------------------------------------------------------------

export function registerProviders(
    worker: SclWorker,
    getFiles: () => Record<string, string>,
    _getActiveFile: () => string,
): monaco.IDisposable {
    const disposables: monaco.IDisposable[] = [];

    // Hover provider
    disposables.push(
        monaco.languages.registerHoverProvider(LANGUAGE_ID, {
            async provideHover(model, position) {
                const file = activeFileForModel(model);
                const result = await worker.hover(
                    getFiles(),
                    file,
                    position.lineNumber - 1,
                    position.column - 1,
                );
                if (!result) return null;
                const contents: monaco.IMarkdownString[] = [
                    { value: `\`\`\`scl\n${result.type}\n\`\`\`` },
                ];
                if (result.description) {
                    contents.push({ value: result.description });
                }
                return { contents };
            },
        }),
    );

    // Completion provider
    disposables.push(
        monaco.languages.registerCompletionItemProvider(LANGUAGE_ID, {
            triggerCharacters: ["."],
            async provideCompletionItems(model, position) {
                const file = activeFileForModel(model);
                const items = await worker.completions(
                    getFiles(),
                    file,
                    position.lineNumber - 1,
                    position.column - 1,
                );
                const word = model.getWordUntilPosition(position);
                const range = new monaco.Range(
                    position.lineNumber,
                    word.startColumn,
                    position.lineNumber,
                    word.endColumn,
                );
                return {
                    suggestions: items.map((item: CompletionItem) => ({
                        label: item.label,
                        kind:
                            item.kind === "field"
                                ? monaco.languages.CompletionItemKind.Field
                                : monaco.languages.CompletionItemKind.Variable,
                        detail: item.detail,
                        documentation: item.description,
                        insertText: item.label,
                        range,
                    })),
                };
            },
        }),
    );

    // Definition provider
    disposables.push(
        monaco.languages.registerDefinitionProvider(LANGUAGE_ID, {
            async provideDefinition(model, position) {
                const file = activeFileForModel(model);
                const loc = await worker.gotoDefinition(
                    getFiles(),
                    file,
                    position.lineNumber - 1,
                    position.column - 1,
                );
                if (!loc) return null;
                const targetUri = loc.file ? fileUri(loc.file) : model.uri;
                return {
                    uri: targetUri,
                    range: new monaco.Range(
                        loc.line + 1,
                        loc.character + 1,
                        loc.end_line + 1,
                        loc.end_character + 1,
                    ),
                };
            },
        }),
    );

    // Formatting provider
    disposables.push(
        monaco.languages.registerDocumentFormattingEditProvider(LANGUAGE_ID, {
            async provideDocumentFormattingEdits(model) {
                const formatted = await worker.format(model.getValue());
                if (!formatted) return [];
                const fullRange = model.getFullModelRange();
                return [{ range: fullRange, text: formatted }];
            },
        }),
    );

    return {
        dispose() {
            for (const d of disposables) d.dispose();
        },
    };
}

function activeFileForModel(model: monaco.editor.ITextModel): string {
    // Extract file path from model URI: file:///Main.scl -> Main.scl
    const path = model.uri.path;
    return path.startsWith("/") ? path.slice(1) : path;
}

// ---------------------------------------------------------------------------
// Diagnostics (multi-file aware)
// ---------------------------------------------------------------------------

export function setupDiagnostics(
    worker: SclWorker,
    getFiles: () => Record<string, string>,
    onDiagnostics: (diags: DiagnosticInfo[]) => void,
): monaco.IDisposable {
    let timeout: ReturnType<typeof setTimeout> | undefined;
    let lastFilesSnapshot = "";

    const updateDiagnostics = async () => {
        const files = getFiles();
        const snapshot = JSON.stringify(files);
        lastFilesSnapshot = snapshot;

        const diags = await worker.analyze(files);

        // Only update if files haven't changed while we were analyzing
        if (JSON.stringify(getFiles()) !== snapshot) return;

        // Clear all existing markers
        for (const model of monaco.editor.getModels()) {
            if (model.getLanguageId() === LANGUAGE_ID) {
                monaco.editor.setModelMarkers(model, "scl", []);
            }
        }

        // Group diagnostics by file and set markers
        const byFile = new Map<string, DiagnosticInfo[]>();
        for (const d of diags) {
            const existing = byFile.get(d.file) ?? [];
            existing.push(d);
            byFile.set(d.file, existing);
        }

        for (const [file, fileDiags] of byFile) {
            const model = getModel(file);
            if (!model) continue;
            monaco.editor.setModelMarkers(
                model,
                "scl",
                fileDiags.map((d) => ({
                    startLineNumber: d.line + 1,
                    startColumn: d.character + 1,
                    endLineNumber: d.end_line + 1,
                    endColumn: d.end_character + 1,
                    message: d.message,
                    severity:
                        d.severity === "error"
                            ? monaco.MarkerSeverity.Error
                            : monaco.MarkerSeverity.Warning,
                })),
            );
        }

        onDiagnostics(diags);
    };

    const schedule = () => {
        clearTimeout(timeout);
        timeout = setTimeout(updateDiagnostics, 300);
    };

    // Run immediately on setup
    updateDiagnostics();

    // Listen to content changes on ALL scl models
    const disposables: monaco.IDisposable[] = [];

    // Listen for new models being created
    disposables.push(
        monaco.editor.onDidCreateModel((model) => {
            if (model.getLanguageId() === LANGUAGE_ID) {
                disposables.push(model.onDidChangeContent(schedule));
            }
        }),
    );

    // Also listen on existing models
    for (const model of monaco.editor.getModels()) {
        if (model.getLanguageId() === LANGUAGE_ID) {
            disposables.push(model.onDidChangeContent(schedule));
        }
    }

    return {
        dispose() {
            clearTimeout(timeout);
            for (const d of disposables) d.dispose();
        },
    };
}

// ---------------------------------------------------------------------------
// Editor creation
// ---------------------------------------------------------------------------

const THEME_ID = "github-light";

monaco.editor.defineTheme(THEME_ID, {
    base: "vs",
    inherit: false,
    rules: [
        { token: "", foreground: "24292e" },
        { token: "comment", foreground: "6a737d", fontStyle: "italic" },
        { token: "keyword", foreground: "d73a49" },
        { token: "constant", foreground: "005cc5" },
        { token: "string", foreground: "032f62" },
        { token: "string.escape", foreground: "005cc5" },
        { token: "string.interpolation", foreground: "d73a49" },
        { token: "number", foreground: "005cc5" },
        { token: "operator", foreground: "d73a49" },
        { token: "type.identifier", foreground: "6f42c1" },
        { token: "identifier", foreground: "24292e" },
        { token: "delimiter", foreground: "24292e" },
        { token: "delimiter.bracket", foreground: "24292e" },
    ],
    colors: {
        "editor.background": "#ffffff",
        "editor.foreground": "#24292e",
        "editor.lineHighlightBackground": "#f6f8fa",
        "editorLineNumber.foreground": "#babbbc",
        "editorLineNumber.activeForeground": "#24292e",
        "editor.selectionBackground": "#0366d625",
        "editor.inactiveSelectionBackground": "#0366d611",
        "editorCursor.foreground": "#24292e",
        "editorWhitespace.foreground": "#d1d5da",
        "editorIndentGuide.background": "#eff2f5",
        "editorIndentGuide.activeBackground": "#d7dbe0",
    },
});

export function createEditor(
    container: HTMLElement,
    model: monaco.editor.ITextModel,
): monaco.editor.IStandaloneCodeEditor {
    return monaco.editor.create(container, {
        model,
        language: LANGUAGE_ID,
        theme: THEME_ID,
        automaticLayout: false,
        minimap: { enabled: false },
        fontSize: 12,
        lineHeight: 20,
        lineNumbers: "on",
        renderWhitespace: "selection",
        scrollBeyondLastLine: false,
        padding: { top: 16 },
    });
}
