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
            "true",
            "false",
            "type",
            "as",
        ],
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
                // Strings
                [/"/, "string", "@string"],
                // Numbers
                [/\b\d+(\.\d+)?\b/, "number"],
                // Keywords and identifiers
                [
                    /\b[a-zA-Z_]\w*\b/,
                    {
                        cases: {
                            "@keywords": "keyword",
                            "@default": "identifier",
                        },
                    },
                ],
                // Uppercase identifiers (types/modules)
                [/\b[A-Z]\w*\b/, "type.identifier"],
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

export function registerProviders(worker: SclWorker): monaco.IDisposable {
    const disposables: monaco.IDisposable[] = [];

    // Hover provider
    disposables.push(
        monaco.languages.registerHoverProvider(LANGUAGE_ID, {
            async provideHover(model, position) {
                const result = await worker.hover(
                    model.getValue(),
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
                const items = await worker.completions(
                    model.getValue(),
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
                const loc = await worker.gotoDefinition(
                    model.getValue(),
                    position.lineNumber - 1,
                    position.column - 1,
                );
                if (!loc) return null;
                return {
                    uri: model.uri,
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

export function setupDiagnostics(
    editor: monaco.editor.IStandaloneCodeEditor,
    worker: SclWorker,
): monaco.IDisposable {
    let timeout: ReturnType<typeof setTimeout> | undefined;

    const updateDiagnostics = async () => {
        const model = editor.getModel();
        if (!model) return;
        const source = model.getValue();
        const diags = await worker.analyze(source);
        // Only update if the source hasn't changed while we were analyzing
        if (model.getValue() !== source) return;
        monaco.editor.setModelMarkers(
            model,
            "scl",
            diags.map((d: DiagnosticInfo) => ({
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
    };

    const schedule = () => {
        clearTimeout(timeout);
        timeout = setTimeout(updateDiagnostics, 300);
    };

    // Run immediately on setup
    updateDiagnostics();

    const disposable = editor.onDidChangeModelContent(schedule);

    return {
        dispose() {
            clearTimeout(timeout);
            disposable.dispose();
        },
    };
}

export function createEditor(
    container: HTMLElement,
    initialValue: string,
): monaco.editor.IStandaloneCodeEditor {
    return monaco.editor.create(container, {
        value: initialValue,
        language: LANGUAGE_ID,
        theme: "vs",
        automaticLayout: true,
        minimap: { enabled: false },
        fontSize: 14,
        lineNumbers: "on",
        renderWhitespace: "selection",
        scrollBeyondLastLine: false,
        padding: { top: 16 },
    });
}
