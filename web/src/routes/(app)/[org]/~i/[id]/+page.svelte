<script lang="ts">
import { page } from "$app/stores";
import Spinner from "$lib/components/Spinner.svelte";
import { OrganizationIncidentDetailDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { deploymentHref, envHref, orgHref, orgIncidentsHref } from "$lib/paths";

let orgName = $derived($page.params.org ?? "");
let incidentId = $derived($page.params.id ?? "");

const detail = graphqlQuery(() => ({
    document: OrganizationIncidentDetailDocument,
    variables: { org: orgName, id: incidentId },
    refetchInterval: 15_000,
}));

let incident = $derived(detail.data?.organization.incident ?? null);
</script>

<svelte:head>
    <title>Incident · {orgName} – Skyr</title>
</svelte:head>

<div
    class="flex items-center justify-between border-b border-gray-200 bg-white px-4 h-10 sticky top-14 z-30"
>
    <span class="text-xs text-gray-500">
        <a href={orgIncidentsHref(orgName)} class="hover:text-gray-700">Incidents</a>
        <span class="mx-1 text-gray-400">/</span>
        <a href={orgHref(orgName)} class="hover:text-gray-700">{orgName}</a>
        <span class="mx-1 text-gray-400">/</span>
        <span class="font-mono">{incidentId.slice(0, 8)}</span>
    </span>
</div>

<div class="p-6">
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
        <div class="flex items-center gap-3">
            <span class="text-sm text-gray-500">
                {incident.closedAt ? "Closed" : "Open"}
            </span>
        </div>

        <dl class="grid grid-cols-1 md:grid-cols-2 gap-x-6 gap-y-2 text-sm">
            <dt class="text-gray-500">Entity</dt>
            <dd class="font-mono text-xs break-all">{incident.entityQid}</dd>

            <dt class="text-gray-500">Opened at</dt>
            <dd>{new Date(incident.openedAt).toLocaleString()}</dd>

            <dt class="text-gray-500">Closed at</dt>
            <dd>
                {incident.closedAt
                    ? new Date(incident.closedAt).toLocaleString()
                    : "—"}
            </dd>

            <dt class="text-gray-500">Last report at</dt>
            <dd>{new Date(incident.lastReportAt).toLocaleString()}</dd>

            <dt class="text-gray-500">Report count</dt>
            <dd>{incident.reportCount}</dd>
        </dl>

        {#if incident.summary}
            <div>
                <h3 class="text-sm font-medium text-gray-700 mb-1">Summary</h3>
                <pre
                    class="bg-gray-50 border border-gray-200 rounded p-3 text-xs whitespace-pre-wrap break-all">{incident.summary}</pre>
            </div>
        {/if}

        {#if incident.deployment && incident.repository && incident.environment}
            {@const repo = incident.repository.name}
            {@const env = incident.environment.name}
            {@const dep = incident.deployment}
            <div>
                <h3 class="text-sm font-medium text-gray-700 mb-1">Deployment</h3>
                <a
                    class="text-orange-600 hover:text-orange-500 text-sm"
                    href={deploymentHref(orgName, repo, env, dep.id)}
                >
                    {repo}/{env} · {dep.commit.hash.slice(0, 8)} —
                    {dep.commit.message.split("\n")[0]}
                </a>
            </div>
        {:else if incident.resource && incident.repository && incident.environment}
            {@const repo = incident.repository.name}
            {@const env = incident.environment.name}
            {@const res = incident.resource}
            <div>
                <h3 class="text-sm font-medium text-gray-700 mb-1">Resource</h3>
                <a
                    class="text-orange-600 hover:text-orange-500 text-sm"
                    href={envHref(orgName, repo, env)}
                >
                    {repo}/{env} · {res.type} "{res.name}"
                </a>
            </div>
        {/if}
    </div>
{/if}
</div>
