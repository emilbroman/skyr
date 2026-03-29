import { Marked } from "marked";
import { createHighlighter, type Highlighter } from "shiki";
import sclGrammar from "$lib/scl.tmLanguage.json";
import rawStdlibTypes from "$lib/stdlib-types.json";

export interface RecordType {
    fields: Record<string, Type>;
    doc_comments: Record<string, string>;
}

export interface Type {
    kind: TypeKind;
    name: string | null;
}

export type TypeKind =
    | "Any"
    | "Int"
    | "Float"
    | "Bool"
    | "Str"
    | "Never"
    | { Optional: Type }
    | { List: Type }
    | { Fn: FnType }
    | { Record: RecordType }
    | { Dict: { key: Type; value: Type } }
    | { IsoRec: [number, Type] }
    | { Var: number }
    | { Exception: number };

export interface FnType {
    type_params: [number, Type][];
    params: Type[];
    ret: Type;
}

interface RawModuleExports {
    value_exports: RecordType;
    type_exports: RecordType;
}

const stdlibTypes = rawStdlibTypes as unknown as Record<string, RawModuleExports>;

/** Convert a PascalCase module name to kebab-case. */
export function toKebab(name: string): string {
    return name
        .replace(/([a-z0-9])([A-Z])/g, "$1-$2")
        .replace(/([A-Z])([A-Z][a-z])/g, "$1-$2")
        .toLowerCase();
}

/** Strip the `Std/` prefix from a module ID. */
function stripPrefix(moduleId: string): string {
    return moduleId.startsWith("Std/") ? moduleId.slice(4) : moduleId;
}

export interface StdlibModule {
    /** Full module ID, e.g. `Std/Artifact` */
    name: string;
    /** Short name after `Std/`, e.g. `Artifact` */
    shortName: string;
    /** Kebab-cased slug, e.g. `artifact` */
    slug: string;
    valueExports: RecordType;
    typeExports: RecordType;
}

/** Get all stdlib modules sorted by name. */
export function getStdlibModules(): StdlibModule[] {
    return Object.entries(stdlibTypes)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([name, raw]) => {
            const shortName = stripPrefix(name);
            return {
                name,
                shortName,
                slug: toKebab(shortName),
                valueExports: raw.value_exports,
                typeExports: raw.type_exports,
            };
        });
}

/** Look up a single module by its kebab slug. */
export function getStdlibModule(slug: string): StdlibModule | undefined {
    return getStdlibModules().find((m) => m.slug === slug);
}

// --- Markdown rendering for doc comments ---

const SCL_LANG = {
    ...sclGrammar,
    id: "scl",
    scopeName: "source.scl",
} as const;

const LANG_ALIASES: Record<string, string> = {
    sh: "bash",
    zsh: "bash",
    ts: "typescript",
    js: "javascript",
};

let highlighterPromise: Promise<Highlighter> | null = null;

function getHighlighter(): Promise<Highlighter> {
    if (!highlighterPromise) {
        highlighterPromise = createHighlighter({
            themes: ["github-light"],
            langs: [
                "javascript",
                "typescript",
                "json",
                "yaml",
                "toml",
                "html",
                "css",
                "markdown",
                "bash",
                "shell",
                "python",
                "rust",
                "go",
                "dockerfile",
                "sql",
                "graphql",
                "xml",
                "ini",
                "diff",
                "plaintext",
                SCL_LANG,
            ],
        });
    }
    return highlighterPromise;
}

function escapeHtml(s: string): string {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

async function createMarked(): Promise<Marked> {
    const hl = await getHighlighter();
    const marked = new Marked();
    marked.use({
        async: true,
        renderer: {
            code({ text, lang }) {
                const language = LANG_ALIASES[lang ?? ""] ?? lang ?? "plaintext";
                try {
                    return hl.codeToHtml(text, {
                        lang: language as Parameters<Highlighter["codeToHtml"]>[1]["lang"],
                        theme: "github-light",
                    });
                } catch {
                    return `<pre><code>${escapeHtml(text)}</code></pre>`;
                }
            },
        },
    });
    return marked;
}

/**
 * Render all `doc_comments` in a RecordType (and nested Record types
 * within function params/returns) from Markdown to HTML.
 */
export async function renderRecordDocs(record: RecordType): Promise<RecordType> {
    const marked = await createMarked();

    const cache = new Map<string, string>();
    async function render(md: string): Promise<string> {
        const cached = cache.get(md);
        if (cached !== undefined) return cached;
        const html = await marked.parse(md);
        cache.set(md, html);
        return html;
    }

    async function processDocComments(
        docs: Record<string, string>,
    ): Promise<Record<string, string>> {
        const result: Record<string, string> = {};
        for (const [key, value] of Object.entries(docs)) {
            result[key] = await render(value);
        }
        return result;
    }

    async function processRecordType(rec: RecordType): Promise<RecordType> {
        const fields: Record<string, Type> = {};
        for (const [name, ty] of Object.entries(rec.fields)) {
            fields[name] = await processType(ty);
        }
        return {
            fields,
            doc_comments: await processDocComments(rec.doc_comments),
        };
    }

    async function processType(ty: Type): Promise<Type> {
        const kind = ty.kind;
        if (typeof kind === "object" && "Record" in kind) {
            return { ...ty, kind: { Record: await processRecordType(kind.Record) } };
        }
        if (typeof kind === "object" && "Fn" in kind) {
            const fn = kind.Fn;
            return {
                ...ty,
                kind: {
                    Fn: {
                        type_params: fn.type_params,
                        params: await Promise.all(fn.params.map(processType)),
                        ret: await processType(fn.ret),
                    },
                },
            };
        }
        if (typeof kind === "object" && "Optional" in kind) {
            return { ...ty, kind: { Optional: await processType(kind.Optional) } };
        }
        if (typeof kind === "object" && "List" in kind) {
            return { ...ty, kind: { List: await processType(kind.List) } };
        }
        if (typeof kind === "object" && "Dict" in kind) {
            return {
                ...ty,
                kind: {
                    Dict: {
                        key: await processType(kind.Dict.key),
                        value: await processType(kind.Dict.value),
                    },
                },
            };
        }
        if (typeof kind === "object" && "IsoRec" in kind) {
            return { ...ty, kind: { IsoRec: [kind.IsoRec[0], await processType(kind.IsoRec[1])] } };
        }
        return ty;
    }

    return processRecordType(record);
}
