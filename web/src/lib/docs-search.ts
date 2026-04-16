import Fuse from "fuse.js";
import type { SearchEntry } from "./search-index";

let fuse: Fuse<SearchEntry> | null = null;
let loadPromise: Promise<void> | null = null;

async function loadIndex(): Promise<void> {
    const res = await fetch("/~docs/search-index.json");
    const entries: SearchEntry[] = await res.json();
    fuse = new Fuse(entries, {
        keys: [
            { name: "title", weight: 1.0 },
            { name: "body", weight: 0.5 },
            { name: "pageTitle", weight: 0.3 },
        ],
        threshold: 0.4,
        ignoreLocation: true,
        minMatchCharLength: 2,
    });
}

/** Start loading the search index (call on focus). */
export function ensureSearchIndex(): void {
    if (!loadPromise) {
        loadPromise = loadIndex();
    }
}

/** Search the index. Returns empty array if index not yet loaded. */
export async function search(query: string): Promise<SearchEntry[]> {
    if (!loadPromise) {
        ensureSearchIndex();
    }
    await loadPromise;
    if (!fuse || !query.trim()) return [];
    return fuse.search(query, { limit: 20 }).map((r) => r.item);
}
