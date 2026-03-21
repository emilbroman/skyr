<script lang="ts">
import { OrganizationsDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import Spinner from "$lib/components/Spinner.svelte";
import { orgHref } from "$lib/paths";

const organizations = graphqlQuery(() => ({
    document: OrganizationsDocument,
}));
</script>

<div class="p-6">
  <h1 class="font-bold text-gray-900 mb-6">Organizations</h1>

  {#if organizations.isPending}
    <Spinner />
  {:else if organizations.error}
    <div class="p-4 bg-red-50 border border-red-200 rounded text-red-600">
      {organizations.error.message}
    </div>
  {:else if organizations.data.organizations.length === 0}
    <div class="text-center py-16">
      <p class="text-gray-500 mb-2">No organizations found.</p>
      <p class="text-gray-400">
        Create an organization to get started.
      </p>
    </div>
  {:else}
    <div class="grid gap-4">
      {#each organizations.data.organizations as org}
        <a
          href={orgHref(org.name)}
          class="block bg-white border border-gray-200 rounded-lg p-5 hover:border-gray-400 transition-colors"
        >
          <div class="flex items-center justify-between">
            <h2 class="font-medium text-gray-900">{org.name}</h2>
            <span class="text-gray-400"
              >{org.repositories.length} repositor{org.repositories.length !== 1
                ? "ies"
                : "y"}</span
            >
          </div>
        </a>
      {/each}
    </div>
  {/if}
</div>
