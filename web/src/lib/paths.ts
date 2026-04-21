/**
 * Encode a segment for use in a URL path.
 * Slashes are replaced with `~` (which is forbidden in Git ref names)
 * rather than percent-encoded as `%2F`, because Traefik rejects `%2F` in paths.
 */
export function encodeSegment(value: string): string {
    return encodeURIComponent(value.replaceAll("/", "~"));
}

/**
 * Decode a URL path segment back to the original value.
 * Reverses the `~` → `/` substitution applied by `encodeSegment`.
 */
export function decodeSegment(value: string): string {
    try {
        return decodeURIComponent(value).replaceAll("~", "/");
    } catch {
        return value.replaceAll("~", "/");
    }
}

export function orgHref(orgName: string): string {
    return `/${encodeURIComponent(orgName)}`;
}

export function newOrgHref(): string {
    return "/~new-org";
}

export function newRepoHref(orgName: string): string {
    return `/${encodeURIComponent(orgName)}/~new-repo`;
}

export function repoHref(orgName: string, repoName: string): string {
    return `/${encodeURIComponent(orgName)}/${encodeURIComponent(repoName)}`;
}

export function envHref(orgName: string, repoName: string, envName: string): string {
    return `/${encodeURIComponent(orgName)}/${encodeURIComponent(repoName)}/${encodeSegment(envName)}`;
}

export function deploymentHref(
    orgName: string,
    repoName: string,
    envName: string,
    deploymentId: string,
): string {
    return `/${encodeURIComponent(orgName)}/${encodeURIComponent(repoName)}/${encodeSegment(envName)}/~d/${encodeURIComponent(deploymentId)}`;
}

export function envDeploymentsHref(orgName: string, repoName: string, envName: string): string {
    return `/${encodeURIComponent(orgName)}/${encodeURIComponent(repoName)}/${encodeSegment(envName)}/~d`;
}

export function envArtifactsHref(orgName: string, repoName: string, envName: string): string {
    return `/${encodeURIComponent(orgName)}/${encodeURIComponent(repoName)}/${encodeSegment(envName)}/~a`;
}

export function envLogsHref(orgName: string, repoName: string, envName: string): string {
    return `/${encodeURIComponent(orgName)}/${encodeURIComponent(repoName)}/${encodeSegment(envName)}/~l`;
}

export function resourcesHref(orgName: string, repoName: string, envName: string): string {
    return `/${encodeURIComponent(orgName)}/${encodeURIComponent(repoName)}/${encodeSegment(envName)}/~r`;
}

export function resourceHref(
    orgName: string,
    repoName: string,
    envName: string,
    resourceId: string,
): string {
    return `/${encodeURIComponent(orgName)}/${encodeURIComponent(repoName)}/${encodeSegment(envName)}/~r/${encodeSegment(resourceId)}`;
}

export function commitTreeHref(
    orgName: string,
    repoName: string,
    commitHash: string,
    path?: string,
): string {
    const base = `/${encodeURIComponent(orgName)}/${encodeURIComponent(repoName)}/~c/${encodeURIComponent(commitHash)}`;
    if (!path) return `${base}/`;
    return `${base}/${path}`;
}
