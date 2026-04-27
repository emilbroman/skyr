<script lang="ts">
import { ResourceMarker } from "$lib/graphql/generated";
import ResourceCardContent from "./ResourceCardContent.svelte";

type ResourceData = {
    type: string;
    name: string;
    markers: ResourceMarker[];
};

let { resource, href }: { resource: ResourceData; href?: string } = $props();
</script>

{#if href}
  <a
    {href}
    class="block bg-white border border-gray-200 rounded-lg p-3 hover:bg-gray-50 transition-colors"
  >
    <ResourceCardContent {resource} />
  </a>
{:else}
  <div class="bg-white border border-gray-200 rounded-lg p-3">
    <ResourceCardContent {resource} />
  </div>
{/if}
