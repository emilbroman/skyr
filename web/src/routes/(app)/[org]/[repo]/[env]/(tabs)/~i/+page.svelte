<script lang="ts">
import { onDestroy } from "svelte";
import { page } from "$app/stores";
import IncidentEntityLink from "$lib/components/IncidentEntityLink.svelte";
import Spinner from "$lib/components/Spinner.svelte";
import { EnvironmentIncidentsDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { decodeSegment, envIncidentHref } from "$lib/paths";
import { formatCompactTimestamp, formatDuration } from "$lib/timestamps";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let envName = $derived(decodeSegment($page.params.env ?? ""));

const incidents = graphqlQuery(() => ({
    document: EnvironmentIncidentsDocument,
    variables: { org: orgName, repo: repoName, env: envName },
    refetchInterval: 15_000,
}));

let rows = $derived(incidents.data?.organization.repository.environment.incidents ?? []);

let now = $state(Date.now());
const tick = setInterval(() => {
    now = Date.now();
}, 1000);
onDestroy(() => clearInterval(tick));

function timeframe(openedAt: string, closedAt: string | null | undefined): string {
    const start = new Date(openedAt).getTime();
    const end = closedAt ? new Date(closedAt).getTime() : now;
    return `${formatCompactTimestamp(openedAt)} (${formatDuration(end - start)})`;
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
    <div class="md:hidden space-y-2">
        {#each rows as incident}
            {@const entity = incident.entity}
            {@const isOpen = incident.closedAt == null}
            <a
                href={envIncidentHref(orgName, repoName, envName, incident.id)}
                class="block bg-white border border-gray-200 rounded-lg p-3 hover:bg-gray-50 transition-colors"
            >
                <div class="flex items-center justify-between text-xs text-gray-500">
                    <span class="font-mono">{incident.id.slice(-8)}</span>
                    <span
                        class="tabular-nums {isOpen ? 'text-red-600 font-bold' : ''}"
                        title={`${new Date(incident.openedAt).toLocaleString()}${incident.closedAt ? ` → ${new Date(incident.closedAt).toLocaleString()}` : ""}`}
                    >
                        {timeframe(incident.openedAt, incident.closedAt)}
                    </span>
                </div>
                <div class="mt-1 text-blue-600 break-words line-clamp-3 whitespace-pre-line">
                    {incident.summary ?? ""}
                </div>
                <div class="mt-2 flex items-center gap-2 text-xs flex-wrap">
                    <span
                        class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-xs font-medium border {isOpen
                            ? 'bg-red-50 text-red-700 border-red-200'
                            : 'bg-gray-100 text-gray-500 border-gray-300'}"
                    >
                        <span
                            class="inline-block w-1.5 h-1.5 rounded-full {isOpen
                                ? 'bg-red-500 animate-pulse'
                                : 'bg-gray-400'}"
                        ></span>
                        {isOpen ? "OPEN" : "CLOSED"}
                    </span>
                    {#if entity}
                        <IncidentEntityLink
                            {entity}
                            org={orgName}
                            repo={repoName}
                            env={envName}
                        />
                    {:else}
                        <span class="text-gray-500 font-mono text-xs">(destroyed)</span>
                    {/if}
                </div>
            </a>
        {/each}
    </div>

    <div class="hidden md:block bg-white border border-gray-200 rounded-lg overflow-x-auto">
        <table class="w-full text-left text-xs">
            <thead>
                <tr class="border-b border-gray-200 text-gray-500 bg-gray-50 whitespace-nowrap">
                    <th class="py-2 pl-4 pr-4 font-semibold text-xs text-gray-700"></th>
                    <th class="py-2 pr-4 font-semibold text-xs text-gray-700 w-full">Incident</th>
                    <th class="py-2 pr-4 font-semibold text-xs text-gray-700">Observed on</th>
                    <th class="py-2 pr-4 font-semibold text-xs text-gray-700">Timeframe</th>
                </tr>
            </thead>
            <tbody class="divide-y divide-gray-100">
                {#each rows as incident}
                    {@const entity = incident.entity}
                    {@const isOpen = incident.closedAt == null}
                    <tr class="hover:bg-gray-50 align-baseline">
                        <td class="py-2 pl-4 pr-4">
                            <span
                                class="inline-block w-2 h-2 rounded-full {isOpen
                                    ? 'bg-red-500 animate-pulse'
                                    : 'bg-gray-400'}"
                                aria-label={isOpen ? "Open" : "Closed"}
                            ></span>
                        </td>
                        <td class="py-2 pr-4 w-full">
                            <a
                                href={envIncidentHref(orgName, repoName, envName, incident.id)}
                                class="group flex items-baseline gap-3"
                                title={incident.id}
                            >
                                <span class="font-mono text-gray-500 shrink-0 group-hover:text-gray-700">{incident.id.slice(-8)}</span>
                                <span class="text-gray-800 group-hover:text-blue-600 break-words line-clamp-3 whitespace-pre-line min-w-0">
                                    {incident.summary ?? ""}
                                </span>
                            </a>
                        </td>
                        <td class="py-2 pr-4 max-w-[40ch]">
                            {#if entity}
                                <IncidentEntityLink
                                    {entity}
                                    org={orgName}
                                    repo={repoName}
                                    env={envName}
                                    class="block truncate"
                                />
                            {:else}
                                <span class="text-gray-500 font-mono text-xs">(destroyed)</span>
                            {/if}
                        </td>
                        <td
                            class="py-2 pr-4 whitespace-nowrap tabular-nums {isOpen
                                ? 'text-red-600 font-bold'
                                : 'text-gray-500'}"
                            title={`${new Date(
                                incident.openedAt,
                            ).toLocaleString()}${incident.closedAt ? ` → ${new Date(incident.closedAt).toLocaleString()}` : ""}`}
                        >
                            {timeframe(incident.openedAt, incident.closedAt)}
                        </td>
                    </tr>
                {/each}
            </tbody>
        </table>
    </div>
{/if}
