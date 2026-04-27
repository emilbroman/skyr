<script lang="ts">
import { ResourceMarker } from "$lib/graphql/generated";
import { Activity } from "lucide-svelte";

type ResourceData = {
    type: string;
    name: string;
    markers: ResourceMarker[];
};

let { resource }: { resource: ResourceData } = $props();

let displayType = $derived.by(() => {
    const dot = resource.type.indexOf(".");
    return dot === -1 ? resource.type : resource.type.slice(dot + 1);
});
</script>

<div class="text-gray-500 font-mono text-xs truncate">{displayType}</div>
<div class="mt-1 truncate text-blue-600">{resource.name}</div>
{#if resource.markers.length > 0}
  <div class="mt-2 flex items-center gap-1 flex-wrap">
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
{/if}
