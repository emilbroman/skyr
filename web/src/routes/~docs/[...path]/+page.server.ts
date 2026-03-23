import { error } from "@sveltejs/kit";
import { findAllDocPaths, loadDocPage } from "$lib/docs";
import type { EntryGenerator, PageServerLoad } from "./$types";

export const entries: EntryGenerator = () => {
    return findAllDocPaths().map((path) => ({ path }));
};

export const load: PageServerLoad = async ({ params }) => {
    try {
        const urlPath = (params.path ?? "").replace(/\/$/, "");
        const page = await loadDocPage(urlPath);
        return page;
    } catch {
        error(404, "Page not found");
    }
};
