// This route serves both directories (trailing slash) and files (no trailing slash),
// so we must opt out of SvelteKit's automatic trailing-slash redirects.
export const trailingSlash = 'ignore';
