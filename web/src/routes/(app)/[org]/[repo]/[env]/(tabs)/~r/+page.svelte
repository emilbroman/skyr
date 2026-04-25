<script lang="ts">
import { page } from "$app/stores";
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
  <div class="flex justify-end mb-3">
    <div
      role="radiogroup"
      aria-label="Resource view mode"
      class="inline-flex rounded-md border border-gray-200 overflow-hidden"
    >
      <button
        type="button"
        role="radio"
        aria-checked={view === "graph"}
        aria-label="Graph view"
        onclick={() => {
            view = "graph";
        }}
        class="px-3 py-1.5 transition-colors cursor-pointer {view === 'graph'
            ? 'bg-orange-600 text-gray-900'
            : 'bg-white text-gray-500 hover:bg-gray-50'}"
      >
        <Network size={16} />
      </button>
      <button
        type="button"
        role="radio"
        aria-checked={view === "list"}
        aria-label="List view"
        onclick={() => {
            view = "list";
        }}
        class="px-3 py-1.5 transition-colors cursor-pointer {view === 'list'
            ? 'bg-orange-600 text-gray-900'
            : 'bg-white text-gray-500 hover:bg-gray-50'}"
      >
        <List size={16} />
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
    <p class="text-gray-500">No resources in this environment.</p>
  {:else}
    <div class="md:hidden space-y-3">
      {#each env.resources as resource}
        <ResourceCardCompact
          {resource}
          href={resourceHref(orgName, repoName, envName, resourceId(resource.type, resource.name))}
        />
      {/each}
    </div>

    <div class="hidden md:block bg-white border border-gray-200 rounded-lg overflow-hidden">
      <table class="w-full text-left">
        <thead>
          <tr class="border-b border-gray-200 text-gray-500">
            <th class="pb-3 pt-3 pl-4 pr-4 font-medium">Type</th>
            <th class="pb-3 pt-3 pr-4 font-medium">Name</th>
            <th class="pb-3 pt-3 pr-4 font-medium">Markers</th>
          </tr>
        </thead>
        <tbody>
          {#each env.resources as resource}
            {@const parts = typeParts(resource.type)}
            <tr class="border-b border-gray-200 hover:bg-gray-50 last:border-b-0">
              <td class="py-3 pl-4 pr-4">
                <span class="text-orange-500/70">{parts.prefix}</span><span
                  class="text-orange-500">{parts.tail}</span
                >
              </td>
              <td class="py-3 pr-4">
                <a
                  href={resourceHref(
                      orgName,
                      repoName,
                      envName,
                      resourceId(resource.type, resource.name),
                  )}
                  class="text-orange-600 hover:text-orange-500"
                >
                  {resource.name}
                </a>
              </td>
              <td class="py-3 pr-4">
                <div class="flex items-center gap-1.5 flex-wrap">
                  {#each resource.markers as marker}
                    <span
                      class="px-1 py-px rounded border {marker ===
                      ResourceMarker.Volatile
                          ? 'border-yellow-300 text-yellow-700'
                          : 'border-blue-300 text-blue-700'}"
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
