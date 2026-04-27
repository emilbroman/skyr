<script lang="ts">
import { Check, Copy } from "lucide-svelte";
import { onDestroy } from "svelte";
import { page } from "$app/stores";
import { copyText } from "$lib/clipboard";
import IncidentEntityLink from "$lib/components/IncidentEntityLink.svelte";
import Spinner from "$lib/components/Spinner.svelte";
import { EnvironmentIncidentDetailDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { decodeSegment } from "$lib/paths";
import { formatDuration } from "$lib/timestamps";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let envName = $derived(decodeSegment($page.params.env ?? ""));
let incidentId = $derived($page.params.id ?? "");

const detail = graphqlQuery(() => ({
    document: EnvironmentIncidentDetailDocument,
    variables: { org: orgName, repo: repoName, env: envName, id: incidentId },
    refetchInterval: 15_000,
}));

let incident = $derived(detail.data?.organization.repository.environment.incident ?? null);

let now = $state(Date.now());
const tick = setInterval(() => {
    now = Date.now();
}, 1000);
onDestroy(() => clearInterval(tick));

let copied = $state(false);
function copyId() {
    copyText(incidentId);
    copied = true;
    setTimeout(() => (copied = false), 2000);
}

let elapsedMs = $derived.by(() => {
    if (!incident) return 0;
    const start = new Date(incident.openedAt).getTime();
    const end = incident.closedAt ? new Date(incident.closedAt).getTime() : now;
    return Math.max(0, end - start);
});
</script>

<svelte:head>
    <title>Incident · {envName} · {repoName} – Skyr</title>
</svelte:head>

{#if detail.isPending}
    <Spinner />
{:else if detail.error}
    <div class="p-4 bg-red-50 border border-red-200 rounded text-red-600">
        {detail.error.message}
    </div>
{:else if !incident}
    <p class="text-gray-500">Incident not found.</p>
{:else}
    <div class="bg-white border border-gray-200 rounded-lg p-4 space-y-3">
        <div class="flex items-baseline justify-between gap-3 flex-wrap">
            <div
                class="flex items-center gap-2 text-sm font-bold {incident.closedAt
                    ? ''
                    : 'text-red-600'}"
            >
                <span
                    class="inline-block w-2.5 h-2.5 rounded-full {incident.closedAt
                        ? 'bg-gray-400'
                        : 'bg-red-500 animate-pulse'}"
                    aria-label={incident.closedAt ? "Closed" : "Open"}
                ></span>
                {incident.closedAt
                    ? `Closed after ${formatDuration(elapsedMs)}`
                    : `Opened ${formatDuration(elapsedMs)} ago`}
            </div>
            <button
                type="button"
                onclick={copyId}
                title="Copy ID"
                class="inline-flex items-center gap-1.5 font-mono text-xs text-gray-500 hover:text-gray-700 break-all cursor-pointer"
            >
                {incidentId}
                {#if copied}
                    <Check class="w-3.5 h-3.5 text-green-500" />
                {:else}
                    <Copy class="w-3.5 h-3.5" />
                {/if}
            </button>
        </div>

        <div class="flex items-baseline justify-between gap-3 flex-wrap">
            {#if incident.entity}
                <p class="text-xs text-gray-500">
                    Observed on
                    <IncidentEntityLink
                        entity={incident.entity}
                        org={orgName}
                        repo={repoName}
                        env={envName}
                    />
                </p>
            {:else}
                <p class="text-xs text-gray-500">
                    The entity this incident was attached to has been destroyed.
                </p>
            {/if}
            <p class="text-xs text-gray-500">
                Reported {incident.reportCount}
                {incident.reportCount === 1 ? "time" : "times"} between
                <span class="text-gray-900">
                    {new Date(incident.openedAt).toLocaleString()}
                </span>
                and
                <span class="text-gray-900">
                    {new Date(incident.lastReportAt).toLocaleString()}
                </span>
            </p>
        </div>

        {#if incident.summary}
            <pre
                class="bg-gray-50 border border-gray-200 rounded p-3 text-xs whitespace-pre-wrap break-all">{incident.summary}</pre>
        {/if}
    </div>
{/if}
