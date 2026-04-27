<script lang="ts">
import { page } from "$app/stores";
import HealthBadge from "$lib/components/HealthBadge.svelte";
import Spinner from "$lib/components/Spinner.svelte";
import {
    HealthStatus,
    IncidentCategory,
    OrganizationIncidentsDocument,
} from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import {
    deploymentHref,
    envHref,
    orgHref,
    orgIncidentHref,
    repoHref,
    resourceHref,
} from "$lib/paths";

let orgName = $derived($page.params.org ?? "");

let categoryFilter = $state<IncidentCategory | "">("");
let openOnly = $state(true);

const incidents = graphqlQuery(() => ({
    document: OrganizationIncidentsDocument,
    variables: {
        org: orgName,
        category: categoryFilter === "" ? null : categoryFilter,
        openOnly,
        limit: 100,
    },
    refetchInterval: 15_000,
}));

let rows = $derived(incidents.data?.organization.incidents ?? []);

function categoryToHealth(category: IncidentCategory): HealthStatus {
    return category === IncidentCategory.Crash ? HealthStatus.Down : HealthStatus.Degraded;
}

function entityCell(incident: (typeof rows)[number]): {
    kind: string;
    name: string;
    href: string | null;
} {
    const repo = incident.repository?.name ?? "";
    const env = incident.environment?.name ?? "";
    if (incident.deployment) {
        const d = incident.deployment;
        return {
            kind: "Deployment",
            name: d.commit.hash.slice(0, 8),
            href: repo && env ? deploymentHref(orgName, repo, env, d.id) : null,
        };
    }
    if (incident.resource) {
        const r = incident.resource;
        const shortType = r.type.split(".").slice(1).join(".") || r.type;
        return {
            kind: shortType,
            name: r.name,
            href: repo && env ? resourceHref(orgName, repo, env, `${r.type}:${r.name}`) : null,
        };
    }
    return { kind: "", name: incident.entityQid, href: null };
}
</script>

<svelte:head>
    <title>Incidents · {orgName} – Skyr</title>
</svelte:head>

<div
    class="flex items-center justify-between border-b border-gray-200 bg-white px-4 h-10 sticky top-14 z-30"
>
    <span class="text-xs text-gray-500">
        Incidents
        <span class="mx-1 text-gray-400">/</span>
        <a href={orgHref(orgName)} class="hover:text-gray-700">{orgName}</a>
    </span>
</div>

<div class="p-6">
<div class="flex flex-wrap gap-3 mb-4 items-center">
    <label class="text-sm text-gray-600 inline-flex items-center gap-2">
        <input type="checkbox" bind:checked={openOnly} />
        Open only
    </label>
    <label class="text-sm text-gray-600 inline-flex items-center gap-2">
        Category
        <select
            bind:value={categoryFilter}
            class="border border-gray-200 rounded px-2 py-1 text-sm"
        >
            <option value="">All</option>
            <option value={IncidentCategory.Crash}>Crash</option>
            <option value={IncidentCategory.SystemError}>System error</option>
            <option value={IncidentCategory.BadConfiguration}>Bad configuration</option>
            <option value={IncidentCategory.CannotProgress}>Cannot progress</option>
            <option value={IncidentCategory.InconsistentState}>Inconsistent state</option>
        </select>
    </label>
</div>

{#if incidents.isPending}
    <Spinner />
{:else if incidents.error}
    <div class="p-4 bg-red-50 border border-red-200 rounded text-red-600">
        {incidents.error.message}
    </div>
{:else if rows.length === 0}
    <p class="text-gray-500">No incidents.</p>
{:else}
    <div class="bg-white border border-gray-200 rounded-lg overflow-x-auto">
        <table class="w-full text-left text-sm">
            <thead>
                <tr class="border-b border-gray-200 text-gray-500">
                    <th class="py-2 pl-4 pr-4 font-medium"></th>
                    <th class="py-2 pr-4 font-medium">Opened</th>
                    <th class="py-2 pr-4 font-medium">Last error</th>
                    <th class="py-2 pr-4 font-medium">Type</th>
                    <th class="py-2 pr-4 font-medium">Reports</th>
                    <th class="py-2 pr-4 font-medium">Repository</th>
                    <th class="py-2 pr-4 font-medium">Environment</th>
                    <th class="py-2 pr-4 font-medium">Entity</th>
                </tr>
            </thead>
            <tbody>
                {#each rows as incident}
                    {@const entity = entityCell(incident)}
                    {@const repoName = incident.repository?.name ?? ""}
                    {@const envName = incident.environment?.name ?? ""}
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
                                href={orgIncidentHref(orgName, incident.id)}
                                class="text-orange-600 hover:text-orange-500 break-words line-clamp-3"
                                title={incident.summary ?? ""}
                            >
                                {incident.summary ?? ""}
                            </a>
                        </td>
                        <td class="py-2 pr-4">
                            <HealthBadge
                                health={categoryToHealth(incident.category)}
                                worstOpenCategory={incident.category}
                                size="small"
                            />
                        </td>
                        <td class="py-2 pr-4 text-gray-500">{incident.reportCount}</td>
                        <td class="py-2 pr-4">
                            {#if repoName}
                                <a
                                    href={repoHref(orgName, repoName)}
                                    class="text-orange-600 hover:text-orange-500"
                                >
                                    {repoName}
                                </a>
                            {:else}
                                <span class="text-gray-400">—</span>
                            {/if}
                        </td>
                        <td class="py-2 pr-4">
                            {#if repoName && envName}
                                <a
                                    href={envHref(orgName, repoName, envName)}
                                    class="text-orange-600 hover:text-orange-500"
                                >
                                    {envName}
                                </a>
                            {:else}
                                <span class="text-gray-400">—</span>
                            {/if}
                        </td>
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
</div>
