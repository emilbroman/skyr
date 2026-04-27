<script lang="ts">
import { ResourceMarker } from "$lib/graphql/generated";

type ResourceData = {
    type: string;
    name: string;
    markers: ResourceMarker[];
};

let { resource, href }: { resource: ResourceData; href?: string } = $props();
let typeParts = $derived(resource.type.split("."));
</script>

{#snippet content()}
  <div class="text-gray-400 font-mono text-xs truncate">
    {#if typeParts.length > 1}
      <span>{typeParts.slice(0, -1).join(".")}.</span>
    {/if}
    <span class="text-gray-700 font-medium">{typeParts[typeParts.length - 1]}</span>
  </div>
  <div class="flex items-center gap-1 mt-0.5 text-xs">
    <span class="text-gray-800 truncate">{resource.name}</span>
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
{/snippet}

{#if href}
  <a
    {href}
    class="block bg-white border border-gray-200 rounded px-3 py-2 hover:bg-gray-50 transition-colors"
  >
    {@render content()}
  </a>
{:else}
  <div class="bg-white border border-gray-200 rounded px-3 py-2">
    {@render content()}
  </div>
{/if}
