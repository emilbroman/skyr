import { getStdlibModules } from "$lib/stdlib";

export const prerender = true;

export const load = () => {
    return { modules: getStdlibModules() };
};
