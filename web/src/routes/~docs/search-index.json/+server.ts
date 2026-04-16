import { json } from "@sveltejs/kit";
import { generateSearchIndex } from "$lib/search-index";
import type { RequestHandler } from "./$types";

export const prerender = true;

export const GET: RequestHandler = () => {
    return json(generateSearchIndex());
};
