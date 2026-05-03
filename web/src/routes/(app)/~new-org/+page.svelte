<script lang="ts">
import { goto } from "$app/navigation";
import RegionSelect from "$lib/components/RegionSelect.svelte";
import { CreateOrganizationDocument } from "$lib/graphql/generated";
import { graphqlMutation } from "$lib/graphql/query";
import { orgHref } from "$lib/paths";

let orgName = $state("");
let region = $state("");
let error = $state<string | null>(null);

const createOrganization = graphqlMutation(CreateOrganizationDocument, {
    onSuccess: (data) => {
        goto(orgHref(data.createOrganization.name));
    },
    onError: (e) => {
        error = e.message;
    },
});

function submit() {
    const name = orgName.trim();
    if (!name || !region) return;
    error = null;
    createOrganization.mutate({ name, region });
}
</script>

<svelte:head>
    <title>New Organization – Skyr</title>
</svelte:head>

<div class="max-w-md mx-auto px-6 py-8">
    <h1 class="text-sm font-semibold text-gray-900 mb-4 pb-3 border-b border-gray-200">New organization</h1>

    <form
        onsubmit={(e) => {
            e.preventDefault();
            submit();
        }}
    >
        <label class="block text-xs font-medium text-gray-500 mb-1" for="org-name">
            Organization name
        </label>
        <input
            id="org-name"
            type="text"
            bind:value={orgName}
            placeholder="MyOrganization"
            pattern="[a-zA-Z_][a-zA-Z0-9_]*"
            title="Must start with a letter or underscore, followed by letters, numbers, or underscores"
            required
            class="w-full px-2.5 py-1.5 text-xs bg-white border border-gray-200 rounded text-gray-900 placeholder-gray-400 focus:outline-none focus:border-blue-500"
        />
        <p class="mt-1 text-xs text-gray-400">
            Must start with a letter or underscore, followed by letters, numbers, or underscores.
        </p>

        <label class="block mt-3 text-xs font-medium text-gray-500 mb-1" for="org-region">
            Region
        </label>
        <RegionSelect id="org-region" bind:value={region} />
        <p class="mt-1 text-xs text-gray-400">
            Skyr region the organization is hosted in.
        </p>

        {#if error}
            <div class="mt-3 p-2 bg-red-50 border border-red-200 rounded text-xs text-red-600">
                {error}
            </div>
        {/if}

        <button
            type="submit"
            disabled={createOrganization.isPending || !orgName.trim() || !region}
            class="mt-4 px-3 py-1.5 text-xs font-medium text-white bg-gray-900 rounded hover:bg-gray-800 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
        >
            {createOrganization.isPending ? "Creating..." : "Create organization"}
        </button>
    </form>
</div>
