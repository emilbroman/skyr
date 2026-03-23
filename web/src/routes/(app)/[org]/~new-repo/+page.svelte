<script lang="ts">
import { goto } from "$app/navigation";
import { page } from "$app/stores";
import { CreateRepositoryDocument } from "$lib/graphql/generated";
import { graphqlMutation } from "$lib/graphql/query";
import { repoHref } from "$lib/paths";

let orgName = $derived($page.params.org ?? "");

let repoName = $state("");
let error = $state<string | null>(null);

const createRepository = graphqlMutation(CreateRepositoryDocument, {
    onSuccess: (data) => {
        goto(repoHref(orgName, data.createRepository.name));
    },
    onError: (e) => {
        error = e.message;
    },
});

function submit() {
    const name = repoName.trim();
    if (!name) return;
    error = null;
    createRepository.mutate({ organization: orgName, repository: name });
}
</script>

<div class="p-6 max-w-lg mx-auto">
    <h1 class="font-bold text-gray-900 mb-6">New repository</h1>

    <form
        onsubmit={(e) => {
            e.preventDefault();
            submit();
        }}
    >
        <label class="block font-medium text-gray-500 mb-1" for="repo-name">
            Repository name
        </label>
        <input
            id="repo-name"
            type="text"
            bind:value={repoName}
            placeholder="MyProject"
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
            disabled={createRepository.isPending || !repoName.trim()}
            class="mt-4 px-4 py-2 bg-orange-600 hover:bg-orange-500 text-gray-900 rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
        >
            {createRepository.isPending ? "Creating..." : "Create repository"}
        </button>
    </form>
</div>
