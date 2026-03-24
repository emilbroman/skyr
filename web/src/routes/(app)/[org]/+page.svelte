<script lang="ts">
import { page } from "$app/stores";
import { OrganizationDetailDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import Spinner from "$lib/components/Spinner.svelte";
import { newRepoHref, repoHref } from "$lib/paths";

let orgName = $derived($page.params.org ?? "");

const orgDetail = graphqlQuery(() => ({
    document: OrganizationDetailDocument,
    variables: { org: orgName },
}));
</script>

<svelte:head>
    <title>{orgName} - Skyr</title>
</svelte:head>

<div class="p-6">
  <div class="flex items-center justify-between mb-6">
    <h1 class="font-bold text-gray-900">{orgName}</h1>
    <a
      href={newRepoHref(orgName)}
      class="px-4 py-2 bg-orange-600 hover:bg-orange-500 text-gray-900 rounded font-medium transition-colors"
    >
      New repository
    </a>
  </div>

  {#if orgDetail.isPending}
    <Spinner />
  {:else if orgDetail.error}
    <div class="p-4 bg-red-50 border border-red-200 rounded text-red-600">
      {orgDetail.error.message}
    </div>
  {:else}
    {@const org = orgDetail.data.organization}
    {#if org.repositories.length === 0}
      <div class="text-center py-16">
        <p class="text-gray-500 mb-2">No repositories found.</p>
        <p class="text-gray-400">
          Push an SCL project to create your first repository.
        </p>
      </div>
    {:else}
      <div class="grid gap-4">
        {#each org.repositories as repo}
          <a
            href={repoHref(orgName, repo.name)}
            class="block bg-white border border-gray-200 rounded-lg p-5 hover:border-gray-400 transition-colors"
          >
            <div class="flex items-center justify-between">
              <h2 class="font-medium text-gray-900">{repo.name}</h2>
              <span class="text-gray-400"
                >{repo.environments.length} environment{repo.environments
                  .length !== 1
                  ? "s"
                  : ""}</span
              >
            </div>
            {#if repo.environments.length > 0}
              <div class="mt-3 flex flex-wrap gap-2">
                {#each repo.environments as env}
                  <span
                    class="px-2 py-1 bg-gray-100 rounded text-gray-500"
                  >
                    {env.name}
                  </span>
                {/each}
              </div>
            {/if}
          </a>
        {/each}
      </div>
    {/if}
  {/if}
</div>
