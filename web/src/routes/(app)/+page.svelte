<script lang="ts">
import { OrganizationsDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import Spinner from "$lib/components/Spinner.svelte";
import { Plus } from "lucide-svelte";
import { orgHref, newOrgHref } from "$lib/paths";

const organizations = graphqlQuery(() => ({
    document: OrganizationsDocument,
}));
</script>

<svelte:head>
    <title>Organizations – Skyr</title>
</svelte:head>

<div class="max-w-4xl mx-auto px-6 py-8">
  <div class="flex items-end justify-between mb-6 pb-3 border-b border-gray-200">
    <h1 class="text-sm font-semibold text-gray-900">Organizations</h1>
    <a
      href={newOrgHref()}
      class="inline-flex items-center gap-1.5 px-2.5 py-1 text-xs font-medium text-gray-700 bg-white border border-gray-200 rounded hover:border-gray-300 hover:text-gray-900 transition-colors"
    >
      <Plus class="w-3.5 h-3.5" />
      New organization
    </a>
  </div>

  {#if organizations.isPending}
    <Spinner />
  {:else if organizations.error}
    <div class="p-3 bg-red-50 border border-red-200 rounded text-xs text-red-600">
      {organizations.error.message}
    </div>
  {:else if organizations.data.organizations.length === 0}
    <div class="text-center py-16 border border-dashed border-gray-200 rounded">
      <p class="text-xs text-gray-500 mb-1">No organizations found.</p>
      <p class="text-xs text-gray-400">Create an organization to get started.</p>
    </div>
  {:else}
    <div class="bg-white border border-gray-200 rounded overflow-hidden divide-y divide-gray-100">
      {#each organizations.data.organizations as org}
        <a
          href={orgHref(org.name)}
          class="flex items-center justify-between px-4 py-2.5 hover:bg-gray-50 transition-colors group"
        >
          <h2 class="text-xs font-medium text-gray-900 group-hover:text-blue-600">{org.name}</h2>
          <span class="text-xs text-gray-400"
            >{org.repositories.length} repositor{org.repositories.length !== 1
              ? "ies"
              : "y"}</span
          >
        </a>
      {/each}
    </div>
  {/if}
</div>
