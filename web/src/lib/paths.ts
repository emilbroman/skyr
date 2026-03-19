/**
 * Encode a segment for use in a URL path.
 * Slashes are replaced with `~` (which is forbidden in Git ref names)
 * rather than percent-encoded as `%2F`, because Traefik rejects `%2F` in paths.
 */
export function encodeSegment(value: string): string {
	return encodeURIComponent(value.replaceAll('/', '~'));
}

/**
 * Decode a URL path segment back to the original value.
 * Reverses the `~` → `/` substitution applied by `encodeSegment`.
 */
export function decodeSegment(value: string): string {
	try {
		return decodeURIComponent(value).replaceAll('~', '/');
	} catch {
		return value.replaceAll('~', '/');
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
