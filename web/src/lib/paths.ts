/**
 * Encode a segment for use in a URL path.
 * This handles names that may contain slashes (e.g., environment names like "staging/eu").
 */
export function encodeSegment(value: string): string {
	return encodeURIComponent(value);
}

/**
 * Decode a URL path segment back to the original value.
 * SvelteKit already decodes params, but we call this explicitly for clarity and safety.
 */
export function decodeSegment(value: string): string {
	try {
		return decodeURIComponent(value);
	} catch {
		return value;
	}
}

export function repoHref(repoName: string): string {
	return `/repos/${encodeSegment(repoName)}`;
}

export function envHref(repoName: string, envName: string): string {
	return `/repos/${encodeSegment(repoName)}/${encodeSegment(envName)}`;
}

export function deploymentHref(repoName: string, envName: string, deploymentId: string): string {
	return `/repos/${encodeSegment(repoName)}/${encodeSegment(envName)}/${encodeSegment(deploymentId)}`;
}
