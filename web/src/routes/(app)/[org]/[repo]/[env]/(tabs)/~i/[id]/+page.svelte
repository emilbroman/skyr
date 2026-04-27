<script lang="ts">
import { page } from "$app/stores";
import Spinner from "$lib/components/Spinner.svelte";
import { EnvironmentIncidentDetailDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { decodeSegment, deploymentHref, envIncidentsHref, resourceHref } from "$lib/paths";

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
</script>

<svelte:head>
    <title>Incident · {envName} · {repoName} – Skyr</title>
</svelte:head>

<div class="text-xs text-gray-500 mb-3">
    <a href={envIncidentsHref(orgName, repoName, envName)} class="hover:text-gray-700">
        Incidents
    </a>
    <span class="mx-1 text-gray-400">/</span>
    <span class="font-mono">{incidentId.slice(0, 8)}</span>
</div>

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

        {#if incident.entity}
            {@const entity = incident.entity}
            <div>
                <h3 class="text-sm font-medium text-gray-700 mb-1">
                    {entity.__typename}
                </h3>
                {#if entity.__typename === "Deployment"}
                    <a
                        class="text-orange-600 hover:text-orange-500 text-sm"
                        href={deploymentHref(orgName, repoName, envName, entity.id)}
                    >
                        {entity.commit.hash.slice(0, 8)} —
                        {entity.commit.message.split("\n")[0]}
                    </a>
                {:else if entity.__typename === "Resource"}
                    <a
                        class="text-orange-600 hover:text-orange-500 text-sm"
                        href={resourceHref(
                            orgName,
                            repoName,
                            envName,
                            `${entity.type}:${entity.name}`,
                        )}
                    >
                        {entity.type} "{entity.name}"
                    </a>
                {/if}
            </div>
        {:else}
            <div>
                <h3 class="text-sm font-medium text-gray-700 mb-1">Entity</h3>
                <p class="text-sm text-gray-500">
                    The entity this incident was attached to has been destroyed.
                </p>
            </div>
        {/if}
    </div>
{/if}
