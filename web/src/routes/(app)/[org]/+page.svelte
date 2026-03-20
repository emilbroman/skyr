<script lang="ts">
import { page } from "$app/stores";
import { OrganizationDetailDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { repoHref } from "$lib/paths";

let orgName = $derived($page.params.org ?? "");

const orgDetail = graphqlQuery(() => ({
    document: OrganizationDetailDocument,
    variables: { org: orgName },
}));
</script>

<div class="p-6">
  <nav class="text-sm text-gray-500 mb-4">
    <span class="text-gray-300">{orgName}</span>
  </nav>

  <h1 class="text-2xl font-bold text-white mb-6">{orgName}</h1>

  {#if orgDetail.isPending}
    <p class="text-gray-400">Loading repositories...</p>
  {:else if orgDetail.error}
    <div class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300">
      {orgDetail.error.message}
    </div>
  {:else}
    {@const org = orgDetail.data.organization}
    {#if org.repositories.length === 0}
      <div class="text-center py-16">
        <p class="text-gray-400 mb-2">No repositories found.</p>
        <p class="text-gray-500 text-sm">
          Push an SCL project to create your first repository.
        </p>
      </div>
    {:else}
      <div class="grid gap-4">
        {#each org.repositories as repo}
          <a
            href={repoHref(orgName, repo.name)}
            class="block bg-gray-900 border border-gray-800 rounded-lg p-5 hover:border-gray-700 transition-colors"
          >
            <div class="flex items-center justify-between">
              <h2 class="text-lg font-medium text-white">{repo.name}</h2>
              <span class="text-sm text-gray-500"
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
                    class="text-xs px-2 py-1 bg-gray-800 rounded text-gray-400"
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
