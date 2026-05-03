<script lang="ts">
import HealthBadge from "$lib/components/HealthBadge.svelte";
import ResourceCardCompact from "$lib/components/ResourceCardCompact.svelte";
import ResourceDag from "$lib/components/ResourceDag.svelte";
import { type HealthStatus, ResourceMarker } from "$lib/graphql/generated";
import { resourceHref } from "$lib/paths";
import { Activity, List, Network } from "lucide-svelte";
import { onMount, type Snippet } from "svelte";

type Resource = {
    region: { id: string };
    type: string;
    name: string;
    markers: ResourceMarker[];
    status: { health: HealthStatus; openIncidentCount: number };
    owner?: { id: string } | null;
    dependencies: { region: { id: string }; type: string; name: string }[];
};

let {
    resources,
    org,
    repo,
    env,
    header,
    emptyMessage = "No resources.",
}: {
    resources: Resource[];
    org: string;
    repo: string;
    env: string;
    header?: Snippet;
    emptyMessage?: string;
} = $props();

let view = $state<"graph" | "list">("graph");

onMount(() => {
    const mq = window.matchMedia("(max-width: 767px)");
    view = mq.matches ? "list" : "graph";
});

function resourceId(region: string, type: string, name: string): string {
    return `${region}:${type}:${name}`;
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

<div class="flex items-center {header ? 'justify-between' : 'justify-end'} mb-2">
  {#if header}{@render header()}{/if}
  {#if resources.length > 0}
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
  {/if}
</div>

{#if view === "graph"}
  <ResourceDag {resources} {org} {repo} {env} />
{:else if resources.length === 0}
  <div class="text-center py-12 border border-dashed border-gray-200 rounded">
    <p class="text-xs text-gray-500">{emptyMessage}</p>
  </div>
{:else}
  <div class="md:hidden space-y-2">
    {#each resources as resource}
      <ResourceCardCompact
        {resource}
        href={resourceHref(org, repo, env, resourceId(resource.region.id, resource.type, resource.name))}
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
        {#each resources as resource}
          {@const parts = typeParts(resource.type)}
          <tr class="hover:bg-gray-50">
            <td class="py-2 pl-4 pr-4 font-mono">
              <span class="text-gray-400">{parts.prefix}</span><span class="text-gray-700">{parts.tail}</span>
            </td>
            <td class="py-2 pr-4">
              <a
                href={resourceHref(org, repo, env, resourceId(resource.region.id, resource.type, resource.name))}
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
                    class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-xs font-medium border {marker ===
                    ResourceMarker.Volatile
                        ? 'bg-orange-50 text-orange-700 border-orange-200'
                        : 'bg-blue-50 text-blue-700 border-blue-200'}"
                  >
                    {#if marker === ResourceMarker.Volatile}<Activity class="w-3 h-3" />{/if}
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
