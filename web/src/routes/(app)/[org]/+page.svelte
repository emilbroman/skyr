<script lang="ts">
import { page } from "$app/stores";
import { OrganizationDetailDocument, AddOrganizationMemberDocument } from "$lib/graphql/generated";
import { graphqlQuery, graphqlMutation } from "$lib/graphql/query";
import Spinner from "$lib/components/Spinner.svelte";
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

    <h2 class="font-medium text-gray-500 mb-3">Repositories</h2>

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

    <div class="mt-10">
      <div class="flex items-center justify-between mb-3">
        <h2 class="font-medium text-gray-500">Members</h2>
        {#if isPersonalOrg}
          <span class="text-gray-400 text-sm">
            <a href={newOrgHref()} class="text-orange-600 hover:underline">Create an organization</a> to invite others
          </span>
        {:else}
          <button
            onclick={() => { showAddMember = !showAddMember; }}
            class="px-3 py-1.5 bg-orange-600 hover:bg-orange-500 text-gray-900 rounded font-medium text-sm transition-colors"
          >
            Add member
          </button>
        {/if}
      </div>

      {#if showAddMember}
        <form
          class="mb-4 p-4 bg-white border border-gray-200 rounded-lg"
          onsubmit={(e) => { e.preventDefault(); submitAddMember(); }}
        >
          <label class="block font-medium text-gray-500 mb-1 text-sm" for="new-member">
            Username
          </label>
          <div class="flex gap-2">
            <input
              id="new-member"
              type="text"
              bind:value={newMemberUsername}
              placeholder="username"
              required
              class="flex-1 px-3 py-2 bg-gray-100 border border-gray-300 rounded text-gray-900 placeholder-gray-400 focus:outline-none focus:border-orange-500"
            />
            <button
              type="submit"
              disabled={addMember.isPending || !newMemberUsername.trim()}
              class="px-4 py-2 bg-orange-600 hover:bg-orange-500 text-gray-900 rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {addMember.isPending ? "Adding..." : "Add"}
            </button>
          </div>
          {#if addMemberError}
            <div class="mt-2 p-3 bg-red-50 border border-red-200 rounded text-red-600 text-sm">
              {addMemberError}
            </div>
          {/if}
        </form>
      {/if}

      <div class="bg-white border border-gray-200 rounded-lg overflow-hidden">
        <table class="w-full">
          <thead>
            <tr class="border-b border-gray-200">
              <th class="text-left px-5 py-3 text-gray-500 font-medium text-sm">Username</th>
            </tr>
          </thead>
          <tbody>
            {#each org.members as member}
              <tr class="border-b border-gray-100 last:border-b-0">
                <td class="px-5 py-3 text-gray-900">{member.username}</td>
              </tr>
            {:else}
              <tr>
                <td class="px-5 py-3 text-gray-400">No members</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </div>
  {/if}
</div>
