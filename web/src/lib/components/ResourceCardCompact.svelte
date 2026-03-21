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
  <div class="text-orange-500/70 truncate">
    {#if typeParts.length > 1}
      <span>{typeParts.slice(0, -1).join(".")}.</span>
    {/if}
    <span class="text-orange-500">{typeParts[typeParts.length - 1]}</span>
  </div>
  <div class="flex items-center gap-1.5 mt-0.5">
    <span class="text-gray-600 truncate">{resource.name}</span>
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
