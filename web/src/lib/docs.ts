import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { Marked } from "marked";
import { createHighlighter, type Highlighter } from "shiki";
import sclGrammar from "$lib/scl.tmLanguage.json";

const SCL_LANG = {
    ...sclGrammar,
    id: "scl",
    scopeName: "source.scl",
} as const;

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

const LANG_ALIASES: Record<string, string> = {
    sh: "bash",
    zsh: "bash",
    ts: "typescript",
    js: "javascript",
};

function resolveDocsDir(): string {
    // In the web/ directory, docs is at ../docs
    // In the container, web is at /app and docs is at /docs
    const fromWeb = join(process.cwd(), "..", "docs");
    try {
        statSync(fromWeb);
        return fromWeb;
    } catch {
        return "/docs";
    }
}

export interface DocPage {
    title: string;
    html: string;
}

export function findAllDocPaths(): string[] {
    const docsDir = resolveDocsDir();
    const paths: string[] = [];

    function walk(dir: string, base: string) {
        for (const entry of readdirSync(dir)) {
            const full = join(dir, entry);
            const rel = base ? `${base}/${entry}` : entry;
            if (statSync(full).isDirectory()) {
                walk(full, rel);
            } else if (entry.endsWith(".md")) {
                let urlPath = rel.replace(/\.md$/, "");
                if (urlPath.endsWith("/index")) urlPath = urlPath.slice(0, -"/index".length);
                if (urlPath === "index") urlPath = "";
                paths.push(urlPath);
            }
        }
    }

    walk(docsDir, "");
    return paths;
}

function resolveFilePath(urlPath: string): string {
    const docsDir = resolveDocsDir();

    // Try exact file first
    const exactPath = join(docsDir, `${urlPath || "index"}.md`);
    try {
        statSync(exactPath);
        return exactPath;
    } catch {
        // Try index.md in directory
        const indexPath = join(docsDir, urlPath, "index.md");
        return indexPath;
    }
}

function resolveDocLink(href: string, docFilePath: string): string {
    // docFilePath is relative to docs/, e.g. "scl/types.md" or "index.md"
    const docDir = docFilePath.replace(/[^/]*$/, ""); // e.g. "scl/" or ""

    // Split off anchor
    const hashIdx = href.indexOf("#");
    const pathPart = hashIdx >= 0 ? href.slice(0, hashIdx) : href;
    const anchor = hashIdx >= 0 ? href.slice(hashIdx) : "";

    if (!pathPart) {
        // Pure anchor link
        return href;
    }

    // Resolve relative path against the doc's directory
    const segments = (docDir + pathPart).split("/");
    const resolved: string[] = [];
    for (const seg of segments) {
        if (seg === "..") resolved.pop();
        else if (seg && seg !== ".") resolved.push(seg);
    }
    let result = resolved.join("/");

    // Strip .md extension
    result = result.replace(/\.md$/, "");
    // Strip trailing /index
    result = result.replace(/\/index$/, "");
    if (result === "index") result = "";

    return `/~docs/${result}${result ? "/" : ""}${anchor}`;
}

export async function loadDocPage(urlPath: string): Promise<DocPage> {
    const filePath = resolveFilePath(urlPath);
    const source = readFileSync(filePath, "utf-8");

    // Determine the doc file's relative path within docs/ for link resolution
    const docsDir = resolveDocsDir();
    const docRelPath = filePath.slice(docsDir.length + 1); // e.g. "scl/types.md"

    const hl = await getHighlighter();

    const marked = new Marked();
    marked.use({
        async: true,
        renderer: {
            heading({ text, depth }) {
                const slug = text
                    .toLowerCase()
                    .replace(/<[^>]*>/g, "")
                    .replace(/[^\w\s-]/g, "")
                    .replace(/\s+/g, "-")
                    .replace(/-+/g, "-")
                    .replace(/^-|-$/g, "");
                return `<h${depth} id="${escapeAttr(slug)}">${text}</h${depth}>`;
            },
            code({ text, lang }) {
                const language = LANG_ALIASES[lang ?? ""] ?? lang ?? "plaintext";
                try {
                    return hl.codeToHtml(text, {
                        lang: language as any,
                        theme: "github-light",
                    });
                } catch {
                    return `<pre><code>${escapeHtml(text)}</code></pre>`;
                }
            },
            link({ href, text }) {
                if (
                    href &&
                    !href.startsWith("http") &&
                    !href.startsWith("#") &&
                    !href.startsWith("/")
                ) {
                    href = resolveDocLink(href, docRelPath);
                }
                return `<a href="${escapeAttr(href ?? "")}">${text}</a>`;
            },
        },
    });

    const html = await marked.parse(source);

    // Extract title from first h1
    const titleMatch = source.match(/^#\s+(.+)$/m);
    const title = titleMatch ? titleMatch[1] : "Documentation";

    return { title, html };
}

function escapeHtml(s: string): string {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function escapeAttr(s: string): string {
    return s.replace(/&/g, "&amp;").replace(/"/g, "&quot;");
}
