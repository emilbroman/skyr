import { readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { findAllDocPaths } from "./docs";
import { getStdlibModules, type RecordType } from "./stdlib";

export interface SearchEntry {
    title: string;
    path: string;
    pageTitle: string;
    body: string;
    type: "page" | "heading";
}

const MAX_BODY = 200;

function truncate(text: string, max: number): string {
    if (text.length <= max) return text;
    return `${text.slice(0, max).replace(/\s\S*$/, "")}\u2026`;
}

/** Strip markdown syntax to plain text. */
function stripMarkdown(md: string): string {
    return (
        md
            // Remove code blocks
            .replace(/```[\s\S]*?```/g, "")
            // Remove inline code
            .replace(/`[^`]*`/g, "")
            // Remove images
            .replace(/!\[[^\]]*\]\([^)]*\)/g, "")
            // Remove links, keep text
            .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
            // Remove headings markers
            .replace(/^#{1,6}\s+/gm, "")
            // Remove bold/italic
            .replace(/[*_]{1,3}([^*_]+)[*_]{1,3}/g, "$1")
            // Remove HTML tags
            .replace(/<[^>]*>/g, "")
            // Collapse whitespace
            .replace(/\s+/g, " ")
            .trim()
    );
}

function slugify(text: string): string {
    return text
        .toLowerCase()
        .replace(/<[^>]*>/g, "")
        .replace(/[^\w\s-]/g, "")
        .replace(/\s+/g, "-")
        .replace(/-+/g, "-")
        .replace(/^-|-$/g, "");
}

interface HeadingSection {
    text: string;
    slug: string;
    depth: number;
    body: string;
}

/** Parse markdown into page title, body, and heading sections. */
function parseMarkdown(source: string): {
    title: string;
    body: string;
    headings: HeadingSection[];
} {
    const lines = source.split("\n");
    let title = "Documentation";
    const headings: HeadingSection[] = [];
    let currentBody: string[] = [];
    let pageBody: string[] = [];
    let inH1 = true;

    for (const line of lines) {
        const headingMatch = line.match(/^(#{1,6})\s+(.+)$/);
        if (headingMatch) {
            const depth = headingMatch[1].length;
            const text = headingMatch[2];

            if (depth === 1 && inH1) {
                title = text;
                inH1 = false;
                continue;
            }
            inH1 = false;

            // Flush previous heading's body
            if (headings.length > 0) {
                headings[headings.length - 1].body = stripMarkdown(currentBody.join("\n"));
            } else {
                pageBody = [...currentBody];
            }
            currentBody = [];

            headings.push({
                text,
                slug: slugify(text),
                depth,
                body: "",
            });
        } else {
            currentBody.push(line);
        }
    }

    // Flush last section
    if (headings.length > 0) {
        headings[headings.length - 1].body = stripMarkdown(currentBody.join("\n"));
    } else {
        pageBody = currentBody;
    }

    const body = stripMarkdown(pageBody.join("\n"));
    return { title, body, headings };
}

function resolveDocsDir(): string {
    const fromWeb = join(process.cwd(), "..", "docs");
    try {
        statSync(fromWeb);
        return fromWeb;
    } catch {
        return "/docs";
    }
}

function resolveFilePath(docsDir: string, urlPath: string): string {
    const exactPath = join(docsDir, `${urlPath || "index"}.md`);
    try {
        statSync(exactPath);
        return exactPath;
    } catch {
        return join(docsDir, urlPath, "index.md");
    }
}

function collectStdlibEntries(): SearchEntry[] {
    const entries: SearchEntry[] = [];
    const modules = getStdlibModules();

    for (const mod of modules) {
        const pagePath = `/~docs/scl/stdlib-ref/${mod.slug}/`;
        const pageTitle = mod.name;

        // Page entry for the module
        entries.push({
            title: pageTitle,
            path: pagePath,
            pageTitle: "",
            body: truncate(
                `Standard library module ${mod.name}. ${describeExports(mod.valueExports, mod.typeExports)}`,
                MAX_BODY,
            ),
            type: "page",
        });

        // Heading entries for each export
        function addRecordEntries(record: RecordType, prefix: string) {
            for (const name of Object.keys(record.fields)) {
                const doc = record.doc_comments[name] ?? "";
                entries.push({
                    title: name,
                    path: `${pagePath}#${prefix}${name.toLowerCase()}`,
                    pageTitle,
                    body: truncate(stripMarkdown(doc), MAX_BODY),
                    type: "heading",
                });
            }
        }

        addRecordEntries(mod.typeExports, "type-");
        addRecordEntries(mod.valueExports, "");
    }

    return entries;
}

function describeExports(values: RecordType, types: RecordType): string {
    const parts: string[] = [];
    const typeNames = Object.keys(types.fields);
    const valueNames = Object.keys(values.fields);
    if (typeNames.length > 0) parts.push(`Types: ${typeNames.join(", ")}`);
    if (valueNames.length > 0) parts.push(`Functions: ${valueNames.join(", ")}`);
    return parts.join(". ");
}

export function generateSearchIndex(): SearchEntry[] {
    const entries: SearchEntry[] = [];
    const docsDir = resolveDocsDir();
    const docPaths = findAllDocPaths();

    for (const urlPath of docPaths) {
        const filePath = resolveFilePath(docsDir, urlPath);
        const source = readFileSync(filePath, "utf-8");
        const parsed = parseMarkdown(source);
        const pagePath = `/~docs/${urlPath}${urlPath ? "/" : ""}`;

        // Page entry
        entries.push({
            title: parsed.title,
            path: pagePath,
            pageTitle: "",
            body: truncate(parsed.body, MAX_BODY),
            type: "page",
        });

        // Heading entries
        for (const heading of parsed.headings) {
            entries.push({
                title: heading.text,
                path: `${pagePath}#${heading.slug}`,
                pageTitle: parsed.title,
                body: truncate(heading.body, MAX_BODY),
                type: "heading",
            });
        }
    }

    // Add stdlib reference entries
    entries.push(...collectStdlibEntries());

    return entries;
}
