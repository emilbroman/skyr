<script lang="ts">
import { page } from "$app/stores";
import Spinner from "$lib/components/Spinner.svelte";
import { EnvironmentIncidentsDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { decodeSegment, deploymentHref, envIncidentHref, resourceHref } from "$lib/paths";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let envName = $derived(decodeSegment($page.params.env ?? ""));

const incidents = graphqlQuery(() => ({
    document: EnvironmentIncidentsDocument,
    variables: { org: orgName, repo: repoName, env: envName },
    refetchInterval: 15_000,
}));

let rows = $derived(incidents.data?.organization.repository.environment.incidents ?? []);

function entityCell(incident: (typeof rows)[number]): {
    kind: string;
    name: string;
    href: string | null;
} {
    const entity = incident.entity;
    if (!entity) {
        return { kind: "", name: "(destroyed)", href: null };
    }
    if (entity.__typename === "Deployment") {
        return {
            kind: "Deployment",
            name: entity.commit.hash.slice(0, 8),
            href: deploymentHref(orgName, repoName, envName, entity.id),
        };
    }
    if (entity.__typename === "Resource") {
        const shortType = entity.type.split(".").slice(1).join(".") || entity.type;
        return {
            kind: shortType,
            name: entity.name,
            href: resourceHref(orgName, repoName, envName, `${entity.type}:${entity.name}`),
        };
    }
    return { kind: "", name: entity.qid, href: null };
}
</script>

<svelte:head>
    <title>Incidents · {envName} · {repoName} – Skyr</title>
</svelte:head>

{#if incidents.isPending}
    <Spinner />
{:else if incidents.error}
    <div class="p-4 bg-red-50 border border-red-200 rounded text-red-600">
        {incidents.error.message}
    </div>
{:else if rows.length === 0}
    <p class="text-gray-500">No incidents in this environment.</p>
{:else}
    <div class="bg-white border border-gray-200 rounded-lg overflow-x-auto">
        <table class="w-full text-left text-sm">
            <thead>
                <tr class="border-b border-gray-200 text-gray-500">
                    <th class="py-2 pl-4 pr-4 font-medium"></th>
                    <th class="py-2 pr-4 font-medium">Opened</th>
                    <th class="py-2 pr-4 font-medium">Last error</th>
                    <th class="py-2 pr-4 font-medium">Reports</th>
                    <th class="py-2 pr-4 font-medium">Entity</th>
                </tr>
            </thead>
            <tbody>
                {#each rows as incident}
                    {@const entity = entityCell(incident)}
                    {@const isOpen = incident.closedAt == null}
                    <tr class="border-b border-gray-200 hover:bg-gray-50">
                        <td class="py-2 pl-4 pr-4">
                            {#if isOpen}
                                <span
                                    class="inline-block w-2 h-2 rounded-full bg-red-500 animate-pulse"
                                    aria-label="Open"
                                ></span>
                            {/if}
                        </td>
                        <td class="py-2 pr-4 text-gray-500">
                            {new Date(incident.openedAt).toLocaleString()}
                        </td>
                        <td class="py-2 pr-4 max-w-[40ch]">
                            <a
                                href={envIncidentHref(orgName, repoName, envName, incident.id)}
                                class="text-orange-600 hover:text-orange-500 break-words line-clamp-3"
                                title={incident.summary ?? ""}
                            >
                                {incident.summary ?? ""}
                            </a>
                        </td>
                        <td class="py-2 pr-4 text-gray-500">{incident.reportCount}</td>
                        <td class="py-2 pr-4 max-w-[40ch] font-mono text-xs">
                            {#if entity.href}
                                <a
                                    href={entity.href}
                                    class="text-orange-600 hover:text-orange-500 block"
                                    title={`${entity.kind} ${entity.name}`}
                                >
                                    {#if entity.kind}
                                        <span class="block truncate">{entity.kind}</span>
                                    {/if}
                                    <span class="block truncate">{entity.name}</span>
                                </a>
                            {:else}
                                <span
                                    class="text-gray-500 block"
                                    title={`${entity.kind} ${entity.name}`}
                                >
                                    {#if entity.kind}
                                        <span class="block truncate">{entity.kind}</span>
                                    {/if}
                                    <span class="block truncate">{entity.name}</span>
                                </span>
                            {/if}
                        </td>
                    </tr>
                {/each}
            </tbody>
        </table>
    </div>
{/if}
