<script lang="ts">
import { AvailableRegionsDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";

type Props = {
    id?: string;
    value: string;
    disabled?: boolean;
    onchange?: (value: string) => void;
};

let { id, value = $bindable(), disabled = false, onchange }: Props = $props();

const regionsQuery = graphqlQuery(() => ({ document: AvailableRegionsDocument }));

let regions = $derived(regionsQuery.data?.availableRegions ?? []);
let loading = $derived(regionsQuery.isLoading);
let queryError = $derived(regionsQuery.error?.message ?? null);

$effect(() => {
    if (!value && regions.length > 0) {
        value = regions[0];
        onchange?.(regions[0]);
    }
});
</script>

<select
    {id}
    bind:value
    onchange={() => onchange?.(value)}
    disabled={disabled || loading || regions.length === 0}
    class="w-full px-2.5 py-1.5 text-xs bg-white border border-gray-200 rounded text-gray-900 focus:outline-none focus:border-blue-500 disabled:bg-gray-50 disabled:text-gray-500"
>
    {#if loading}
        <option value="">Loading regions…</option>
    {:else if regions.length === 0}
        <option value="">No regions available</option>
    {:else}
        {#each regions as region (region)}
            <option value={region}>{region}</option>
        {/each}
    {/if}
</select>
{#if queryError}
    <p class="mt-1 text-xs text-red-600">Failed to load regions: {queryError}</p>
{/if}
