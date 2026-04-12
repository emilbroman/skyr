<script lang="ts">
import { goto } from "$app/navigation";
import { CreateOrganizationDocument } from "$lib/graphql/generated";
import { graphqlMutation } from "$lib/graphql/query";
import { orgHref } from "$lib/paths";

let orgName = $state("");
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
    if (!name) return;
    error = null;
    createOrganization.mutate({ name });
}
</script>

<svelte:head>
    <title>New Organization – Skyr</title>
</svelte:head>

<div class="p-6 max-w-lg mx-auto">
    <h1 class="font-bold text-gray-900 mb-6">New organization</h1>

    <form
        onsubmit={(e) => {
            e.preventDefault();
            submit();
        }}
    >
        <label class="block font-medium text-gray-500 mb-1" for="org-name">
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
            class="w-full px-3 py-2 bg-gray-100 border border-gray-300 rounded text-gray-900 placeholder-gray-400 focus:outline-none focus:border-orange-500"
        />
        <p class="mt-1 text-gray-400">
            Must start with a letter or underscore, followed by letters, numbers, or underscores.
        </p>

        {#if error}
            <div
                class="mt-4 p-3 bg-red-50 border border-red-200 rounded text-red-600"
            >
                {error}
            </div>
        {/if}

        <button
            type="submit"
            disabled={createOrganization.isPending || !orgName.trim()}
            class="mt-4 px-4 py-2 bg-orange-600 hover:bg-orange-500 text-gray-900 rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
        >
            {createOrganization.isPending ? "Creating..." : "Create organization"}
        </button>
    </form>
</div>
