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
  <div class="text-[10px] text-indigo-400/70 truncate">
    {#if typeParts.length > 1}
      <span>{typeParts.slice(0, -1).join(".")}.</span>
    {/if}
    <span class="text-indigo-300">{typeParts[typeParts.length - 1]}</span>
  </div>
  <div class="flex items-center gap-1.5 mt-0.5">
    <span class="text-gray-300 text-xs truncate">{resource.name}</span>
    {#each resource.markers as marker}
      <span
        class="text-[9px] px-1 py-px rounded border {marker ===
        ResourceMarker.Volatile
          ? 'border-yellow-700 text-yellow-400'
          : 'border-blue-700 text-blue-400'}"
      >
        {marker}
      </span>
    {/each}
  </div>
{/snippet}

{#if href}
  <a
    {href}
    class="block bg-gray-900 border border-gray-800 rounded px-3 py-2 hover:bg-gray-800/50 transition-colors"
  >
    {@render content()}
  </a>
{:else}
  <div class="bg-gray-900 border border-gray-800 rounded px-3 py-2">
    {@render content()}
  </div>
{/if}
