<script lang="ts">
import { ArrowUpRight } from "lucide-svelte";
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
    <div class="bg-white border border-gray-200 rounded-lg overflow-x-auto">
        <table class="w-full text-left text-sm">
            <thead>
                <tr class="border-b border-gray-200 text-gray-500 whitespace-nowrap">
                    <th class="py-2 pl-4 pr-4 font-medium"></th>
                    <th class="py-2 pr-4 font-medium">ID</th>
                    <th class="py-2 pr-4 font-medium">Observed on</th>
                    <th class="py-2 pr-4 font-medium w-full">Summary</th>
                    <th class="py-2 pr-4 font-medium">Timeframe</th>
                </tr>
            </thead>
            <tbody>
                {#each rows as incident}
                    {@const entity = incident.entity}
                    {@const isOpen = incident.closedAt == null}
                    <tr class="border-b border-gray-200 hover:bg-gray-50 align-baseline">
                        <td class="py-2 pl-4 pr-4">
                            <span
                                class="inline-block w-2 h-2 rounded-full {isOpen
                                    ? 'bg-red-500 animate-pulse'
                                    : 'bg-gray-400'}"
                                aria-label={isOpen ? "Open" : "Closed"}
                            ></span>
                        </td>
                        <td class="py-2 pr-4 font-mono text-xs whitespace-nowrap">
                            <a
                                href={envIncidentHref(orgName, repoName, envName, incident.id)}
                                class="text-gray-900 hover:text-gray-600"
                                title={incident.id}
                            >
                                {incident.id.slice(-8)}
                                <ArrowUpRight
                                    class="w-3.5 h-3.5 inline-block align-text-bottom -ml-0.5"
                                />
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
                        <td class="py-2 pr-4 w-full">
                            <a
                                href={envIncidentHref(orgName, repoName, envName, incident.id)}
                                class="text-gray-900 hover:text-gray-600 break-words line-clamp-3 whitespace-pre-line"
                                title={incident.summary ?? ""}
                            >
                                {incident.summary ?? ""}
                            </a>
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
