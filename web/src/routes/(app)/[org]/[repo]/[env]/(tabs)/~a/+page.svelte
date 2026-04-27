<script lang="ts">
import { page } from "$app/stores";
import { EnvironmentDetailDocument } from "$lib/graphql/generated";
import Spinner from "$lib/components/Spinner.svelte";
import { graphqlQuery } from "$lib/graphql/query";
import { decodeSegment } from "$lib/paths";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let envName = $derived(decodeSegment($page.params.env ?? ""));

const envDetail = graphqlQuery(() => ({
    document: EnvironmentDetailDocument,
    variables: { org: orgName, repo: repoName, env: envName },
    refetchInterval: 10_000,
}));

let artifacts = $derived(envDetail.data?.organization.repository.environment.artifacts ?? []);
</script>

<svelte:head>
    <title>Artifacts · {orgName}/{repoName} ({envName}) – Skyr</title>
</svelte:head>

<div>
  {#if envDetail.isPending}
    <Spinner />
  {:else if envDetail.error}
    <div class="p-4 bg-red-50 border border-red-200 rounded text-red-600">
      {envDetail.error.message}
    </div>
  {:else if artifacts.length === 0}
    <p class="text-gray-400">No artifacts yet.</p>
  {:else}
    <div class="space-y-2">
      {#each artifacts as artifact}
        <div
          class="bg-white border border-gray-200 rounded-lg px-4 py-3 flex items-center justify-between"
        >
          <div>
            <span class="text-gray-700">{artifact.name}</span>
            <span class="text-gray-400 ml-2">({artifact.mediaType})</span>
          </div>
          <a
            href={artifact.url}
            target="_blank"
            rel="noopener noreferrer"
            class="text-blue-600 hover:text-blue-500"
          >
            Download
          </a>
        </div>
      {/each}
    </div>
  {/if}
</div>
