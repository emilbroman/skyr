<script lang="ts">
import { page } from "$app/stores";
import { OrganizationDetailDocument, AddOrganizationMemberDocument } from "$lib/graphql/generated";
import { graphqlQuery, graphqlMutation } from "$lib/graphql/query";
import Spinner from "$lib/components/Spinner.svelte";
import { Plus } from "lucide-svelte";
import { newRepoHref, repoHref, newOrgHref } from "$lib/paths";
import { user } from "$lib/stores/auth";

let orgName = $derived($page.params.org ?? "");
let isPersonalOrg = $derived(orgName === $user?.username);

const orgDetail = graphqlQuery(() => ({
    document: OrganizationDetailDocument,
    variables: { org: orgName },
}));

let newMemberUsername = $state("");
let addMemberError = $state<string | null>(null);
let showAddMember = $state(false);

const addMember = graphqlMutation(AddOrganizationMemberDocument, {
    onSuccess: () => {
        newMemberUsername = "";
        addMemberError = null;
        showAddMember = false;
        orgDetail.refetch();
    },
    onError: (e) => {
        addMemberError = e.message;
    },
});

function submitAddMember() {
    const username = newMemberUsername.trim();
    if (!username) return;
    addMemberError = null;
    addMember.mutate({ organization: orgName, username });
}
</script>

<svelte:head>
    <title>{orgName} – Skyr</title>
</svelte:head>

<div class="max-w-4xl mx-auto px-6 py-8">
  <div class="flex items-end justify-between mb-6 pb-3 border-b border-gray-200">
    <h1 class="text-sm font-semibold text-gray-900">{orgName}</h1>
    <a
      href={newRepoHref(orgName)}
      class="inline-flex items-center gap-1.5 px-2.5 py-1 text-xs font-medium text-gray-700 bg-white border border-gray-200 rounded hover:border-gray-300 hover:text-gray-900 transition-colors"
    >
      <Plus class="w-3.5 h-3.5" />
      New repository
    </a>
  </div>

  {#if orgDetail.isPending}
    <Spinner />
  {:else if orgDetail.error}
    <div class="p-3 bg-red-50 border border-red-200 rounded text-xs text-red-600">
      {orgDetail.error.message}
    </div>
  {:else}
    {@const org = orgDetail.data.organization}

    <div class="mb-8">
      <h2 class="text-xs font-semibold text-gray-700 mb-2">Repositories</h2>

      {#if org.repositories.length === 0}
        <div class="text-center py-12 border border-dashed border-gray-200 rounded">
          <p class="text-xs text-gray-500 mb-1">No repositories found.</p>
          <p class="text-xs text-gray-400">Push an SCL project to create your first repository.</p>
        </div>
      {:else}
        <div class="bg-white border border-gray-200 rounded overflow-hidden divide-y divide-gray-100">
          {#each org.repositories as repo}
            <a
              href={repoHref(orgName, repo.name)}
              class="block px-4 py-2.5 hover:bg-gray-50 transition-colors group"
            >
              <div class="flex items-center justify-between">
                <h3 class="text-xs font-medium text-gray-900 group-hover:text-blue-600">{repo.name}</h3>
                <span class="text-xs text-gray-400"
                  >{repo.environments.length} environment{repo.environments.length !== 1 ? "s" : ""}</span
                >
              </div>
              {#if repo.environments.length > 0}
                <div class="mt-1.5 flex flex-wrap gap-1">
                  {#each repo.environments as env}
                    <span
                      class="px-1.5 py-0.5 text-xs bg-gray-100 rounded text-gray-600"
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
    </div>

    <div>
      <div class="flex items-center justify-between mb-2">
        <h2 class="text-xs font-semibold text-gray-700">Members</h2>
        {#if isPersonalOrg}
          <span class="text-xs text-gray-400">
            <a href={newOrgHref()} class="text-gray-600 hover:text-blue-600 underline-offset-2 hover:underline">Create an organization</a> to invite others
          </span>
        {:else}
          <button
            onclick={() => { showAddMember = !showAddMember; }}
            class="inline-flex items-center gap-1.5 px-2.5 py-1 text-xs font-medium text-gray-700 bg-white border border-gray-200 rounded hover:border-gray-300 hover:text-gray-900 transition-colors"
          >
            <Plus class="w-3.5 h-3.5" />
            Add member
          </button>
        {/if}
      </div>

      {#if showAddMember}
        <form
          class="mb-3 p-3 bg-white border border-gray-200 rounded"
          onsubmit={(e) => { e.preventDefault(); submitAddMember(); }}
        >
          <label class="block text-xs font-medium text-gray-500 mb-1" for="new-member">
            Username
          </label>
          <div class="flex gap-2">
            <input
              id="new-member"
              type="text"
              bind:value={newMemberUsername}
              placeholder="username"
              required
              class="flex-1 px-2.5 py-1.5 text-xs bg-white border border-gray-200 rounded text-gray-900 placeholder-gray-400 focus:outline-none focus:border-blue-500"
            />
            <button
              type="submit"
              disabled={addMember.isPending || !newMemberUsername.trim()}
              class="px-3 py-1.5 text-xs font-medium text-white bg-gray-900 rounded hover:bg-gray-800 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {addMember.isPending ? "Adding..." : "Add"}
            </button>
          </div>
          {#if addMemberError}
            <div class="mt-2 p-2 bg-red-50 border border-red-200 rounded text-xs text-red-600">
              {addMemberError}
            </div>
          {/if}
        </form>
      {/if}

      <div class="bg-white border border-gray-200 rounded overflow-hidden divide-y divide-gray-100">
        {#each org.members as member}
          <div class="px-4 py-2 text-xs text-gray-900">{member.username}</div>
        {:else}
          <div class="px-4 py-2 text-xs text-gray-400">No members</div>
        {/each}
      </div>
    </div>
  {/if}
</div>
