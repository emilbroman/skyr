<script lang="ts">
import { page } from "$app/stores";
import HealthBadge from "$lib/components/HealthBadge.svelte";
import ResourceCardCompact from "$lib/components/ResourceCardCompact.svelte";
import ResourceDag from "$lib/components/ResourceDag.svelte";
import { EnvironmentDetailDocument, ResourceMarker } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { decodeSegment, resourceHref } from "$lib/paths";
import { List, Network } from "lucide-svelte";
import { onMount } from "svelte";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let envName = $derived(decodeSegment($page.params.env ?? ""));

const envDetail = graphqlQuery(() => ({
    document: EnvironmentDetailDocument,
    variables: { org: orgName, repo: repoName, env: envName },
    refetchInterval: 10_000,
}));

let env = $derived(envDetail.data?.organization.repository.environment ?? null);

let view = $state<"graph" | "list">("graph");

onMount(() => {
    const mq = window.matchMedia("(max-width: 767px)");
    view = mq.matches ? "list" : "graph";
});

function resourceId(type: string, name: string): string {
    return `${type}:${name}`;
}

function typeParts(type: string): { prefix: string; tail: string } {
    const parts = type.split(".");
    if (parts.length > 1) {
        return {
            prefix: `${parts.slice(0, -1).join(".")}.`,
            tail: parts[parts.length - 1],
        };
    }
    return { prefix: "", tail: type };
}
</script>

<svelte:head>
    <title>Resources · {orgName}/{repoName} ({envName}) – Skyr</title>
</svelte:head>

{#if env}
  <div class="flex justify-end mb-2">
    <div
      role="radiogroup"
      aria-label="Resource view mode"
      class="inline-flex rounded border border-gray-200 overflow-hidden bg-white"
    >
      <button
        type="button"
        role="radio"
        aria-checked={view === "graph"}
        aria-label="Graph view"
        onclick={() => {
            view = "graph";
        }}
        class="px-2 py-1 transition-colors cursor-pointer {view === 'graph'
            ? 'bg-gray-100 text-gray-900'
            : 'bg-white text-gray-500 hover:bg-gray-50'}"
      >
        <Network size={14} />
      </button>
      <button
        type="button"
        role="radio"
        aria-checked={view === "list"}
        aria-label="List view"
        onclick={() => {
            view = "list";
        }}
        class="px-2 py-1 transition-colors cursor-pointer border-l border-gray-200 {view === 'list'
            ? 'bg-gray-100 text-gray-900'
            : 'bg-white text-gray-500 hover:bg-gray-50'}"
      >
        <List size={14} />
      </button>
    </div>
  </div>

  {#if view === "graph"}
    <ResourceDag
      resources={env.resources}
      org={orgName}
      repo={repoName}
      env={envName}
    />
  {:else if env.resources.length === 0}
    <div class="text-center py-12 border border-dashed border-gray-200 rounded">
      <p class="text-xs text-gray-500">No resources in this environment.</p>
    </div>
  {:else}
    <div class="md:hidden space-y-2">
      {#each env.resources as resource}
        <ResourceCardCompact
          {resource}
          href={resourceHref(orgName, repoName, envName, resourceId(resource.type, resource.name))}
        />
      {/each}
    </div>

    <div class="hidden md:block bg-white border border-gray-200 rounded overflow-hidden">
      <table class="w-full text-left text-xs">
        <thead>
          <tr class="border-b border-gray-200 text-gray-500 bg-gray-50">
            <th class="py-2 pl-4 pr-4 font-semibold text-gray-700">Type</th>
            <th class="py-2 pr-4 font-semibold text-gray-700">Name</th>
            <th class="py-2 pr-4 font-semibold text-gray-700">Health</th>
            <th class="py-2 pr-4 font-semibold text-gray-700">Markers</th>
          </tr>
        </thead>
        <tbody class="divide-y divide-gray-100">
          {#each env.resources as resource}
            {@const parts = typeParts(resource.type)}
            <tr class="hover:bg-gray-50">
              <td class="py-2 pl-4 pr-4 font-mono">
                <span class="text-gray-400">{parts.prefix}</span><span class="text-gray-700">{parts.tail}</span>
              </td>
              <td class="py-2 pr-4">
                <a
                  href={resourceHref(
                      orgName,
                      repoName,
                      envName,
                      resourceId(resource.type, resource.name),
                  )}
                  class="text-gray-800 hover:text-blue-600"
                >
                  {resource.name}
                </a>
              </td>
              <td class="py-2 pr-4">
                <HealthBadge
                  health={resource.status.health}
                  openIncidentCount={resource.status.openIncidentCount}
                  size="small"
                />
              </td>
              <td class="py-2 pr-4">
                <div class="flex items-center gap-1 flex-wrap">
                  {#each resource.markers as marker}
                    <span
                      class="px-1.5 py-0.5 rounded text-xs {marker ===
                      ResourceMarker.Volatile
                          ? 'bg-yellow-50 text-yellow-700 border border-yellow-200'
                          : 'bg-blue-50 text-blue-700 border border-blue-200'}"
                    >
                      {marker}
                    </span>
                  {/each}
                </div>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
{/if}
