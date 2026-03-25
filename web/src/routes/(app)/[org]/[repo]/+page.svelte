<script lang="ts">
import { goto } from "$app/navigation";
import { page } from "$app/stores";
import Spinner from "$lib/components/Spinner.svelte";
import { RepositoryDetailDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { envHref } from "$lib/paths";
import { user } from "$lib/stores/auth";
import { Check, Copy } from "lucide-svelte";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let cloneUrl = $derived(`${$user?.username ?? "git"}@${$page.url.hostname}:${orgName}/${repoName}`);
let copiedClone = $state(false);
let copiedRemote = $state(false);

function copyText(text: string, which: "clone" | "remote") {
    navigator.clipboard.writeText(text);
    if (which === "clone") {
        copiedClone = true;
        setTimeout(() => (copiedClone = false), 2000);
    } else {
        copiedRemote = true;
        setTimeout(() => (copiedRemote = false), 2000);
    }
}

const repoDetail = graphqlQuery(() => ({
    document: RepositoryDetailDocument,
    variables: { org: orgName, repo: repoName },
}));

let repo = $derived(repoDetail.data?.organization.repository ?? null);

let targetEnv = $derived(
    repo?.environments.find((e) => e.name === "main") ?? repo?.environments[0] ?? null,
);

$effect(() => {
    if (targetEnv) {
        goto(envHref(orgName, repoName, targetEnv.name), { replaceState: true });
    }
});
</script>

<svelte:head>
    <title>{orgName}/{repoName} – Skyr</title>
</svelte:head>

{#if repoDetail.isPending}
    <Spinner />
{:else if repoDetail.error}
    <div class="p-4 bg-red-50 border border-red-200 rounded text-red-600">
        {repoDetail.error.message}
    </div>
{:else if repo && repo.environments.length === 0}
    <div class="max-w-lg">
        <h2 class="text-lg font-semibold text-gray-900 mb-1">Get started</h2>
        <p class="text-gray-500 mb-6">
            Push code to this repository to create your first environment.
        </p>

        <div class="space-y-6">
            <div>
                <h3 class="text-sm font-medium text-gray-700 mb-2">Clone this repository</h3>
                <div class="flex items-center gap-2">
                    <code
                        class="flex-1 text-xs bg-gray-50 border border-gray-200 rounded px-3 py-2 text-gray-800 overflow-x-auto whitespace-nowrap"
                    >
                        git clone {cloneUrl}
                    </code>
                    <button
                        class="shrink-0 p-1.5 rounded hover:bg-gray-100 transition-colors text-gray-500 hover:text-gray-700"
                        title="Copy to clipboard"
                        onclick={() => copyText(`git clone ${cloneUrl}`, "clone")}
                    >
                        {#if copiedClone}
                            <Check class="w-4 h-4 text-green-500" />
                        {:else}
                            <Copy class="w-4 h-4" />
                        {/if}
                    </button>
                </div>
            </div>

            <div class="flex items-center gap-3 text-gray-400 text-xs font-medium">
                <div class="flex-1 border-t border-gray-200"></div>
                or
                <div class="flex-1 border-t border-gray-200"></div>
            </div>

            <div>
                <h3 class="text-sm font-medium text-gray-700 mb-2">Add a remote to an existing repository</h3>
                <div class="flex items-center gap-2">
                    <code
                        class="flex-1 text-xs bg-gray-50 border border-gray-200 rounded px-3 py-2 text-gray-800 overflow-x-auto whitespace-nowrap"
                    >
                        git remote add origin {cloneUrl}
                    </code>
                    <button
                        class="shrink-0 p-1.5 rounded hover:bg-gray-100 transition-colors text-gray-500 hover:text-gray-700"
                        title="Copy to clipboard"
                        onclick={() => copyText(`git remote add origin ${cloneUrl}`, "remote")}
                    >
                        {#if copiedRemote}
                            <Check class="w-4 h-4 text-green-500" />
                        {:else}
                            <Copy class="w-4 h-4" />
                        {/if}
                    </button>
                </div>
            </div>
        </div>
    </div>
{/if}
