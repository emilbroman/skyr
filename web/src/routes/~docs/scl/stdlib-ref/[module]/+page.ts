import { error } from "@sveltejs/kit";
import { getStdlibModule, getStdlibModules, renderRecordDocs } from "$lib/stdlib";
import type { EntryGenerator, PageLoad } from "./$types";

export const prerender = true;

export const entries: EntryGenerator = () => {
    return getStdlibModules().map((m) => ({ module: m.slug }));
};

export const load: PageLoad = async ({ params }) => {
    const mod = getStdlibModule(params.module);
    if (!mod) {
        error(404, "Module not found");
    }
    const [valueExports, typeExports] = await Promise.all([
        renderRecordDocs(mod.valueExports),
        renderRecordDocs(mod.typeExports),
    ]);
    return { name: mod.name, shortName: mod.shortName, slug: mod.slug, valueExports, typeExports };
};
