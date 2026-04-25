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
import { envHref, orgHref, orgIncidentHref, repoHref } from "$lib/paths";

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

function entityLabel(incident: (typeof rows)[number]): { label: string; href: string | null } {
    if (incident.deployment) {
        const d = incident.deployment;
        const env = incident.environment?.name ?? "";
        const repo = incident.repository?.name ?? "";
        return {
            label: `${repo}/${env}: deployment ${d.commit.hash.slice(0, 8)}`,
            href: envHref(orgName, repo, env),
        };
    }
    if (incident.resource) {
        const r = incident.resource;
        const env = incident.environment?.name ?? "";
        const repo = incident.repository?.name ?? "";
        return {
            label: `${repo}/${env}: ${r.type} "${r.name}"`,
            href: envHref(orgName, repo, env),
        };
    }
    return { label: incident.entityQid, href: null };
}
</script>

<svelte:head>
    <title>Incidents · {orgName} – Skyr</title>
</svelte:head>

<nav class="text-xl font-bold text-gray-900 mb-3">
    <a href={orgHref(orgName)} class="hover:text-gray-700">{orgName}</a>
    <span class="mx-1 text-gray-400">/</span>
    <span>Incidents</span>
</nav>

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
    <div class="bg-white border border-gray-200 rounded-lg overflow-hidden">
        <table class="w-full text-left text-sm">
            <thead>
                <tr class="border-b border-gray-200 text-gray-500">
                    <th class="py-2 pl-4 pr-4 font-medium">Status</th>
                    <th class="py-2 pr-4 font-medium">Entity</th>
                    <th class="py-2 pr-4 font-medium">Opened</th>
                    <th class="py-2 pr-4 font-medium">Closed</th>
                    <th class="py-2 pr-4 font-medium">Reports</th>
                    <th class="py-2 pr-4 font-medium">Last error</th>
                </tr>
            </thead>
            <tbody>
                {#each rows as incident}
                    {@const entity = entityLabel(incident)}
                    <tr class="border-b border-gray-200 hover:bg-gray-50">
                        <td class="py-2 pl-4 pr-4">
                            <a
                                href={orgIncidentHref(orgName, incident.id)}
                                class="inline-block"
                            >
                                <HealthBadge
                                    health={categoryToHealth(incident.category)}
                                    worstOpenCategory={incident.category}
                                    size="small"
                                />
                            </a>
                        </td>
                        <td class="py-2 pr-4">
                            {#if entity.href}
                                <a
                                    href={entity.href}
                                    class="text-orange-600 hover:text-orange-500"
                                >
                                    {entity.label}
                                </a>
                            {:else}
                                <span class="font-mono text-xs text-gray-500"
                                    >{entity.label}</span
                                >
                            {/if}
                        </td>
                        <td class="py-2 pr-4 text-gray-500">
                            {new Date(incident.openedAt).toLocaleString()}
                        </td>
                        <td class="py-2 pr-4 text-gray-500">
                            {incident.closedAt
                                ? new Date(incident.closedAt).toLocaleString()
                                : "—"}
                        </td>
                        <td class="py-2 pr-4 text-gray-500">{incident.reportCount}</td>
                        <td class="py-2 pr-4 text-gray-500 truncate max-w-xs">
                            {incident.lastErrorMessage ?? ""}
                        </td>
                    </tr>
                {/each}
            </tbody>
        </table>
    </div>
{/if}
