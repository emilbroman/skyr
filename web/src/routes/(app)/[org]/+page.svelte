<script lang="ts">
import { onDestroy } from "svelte";
import { page } from "$app/stores";
import { OrganizationDetailDocument, AddOrganizationMemberDocument } from "$lib/graphql/generated";
import { graphqlQuery, graphqlMutation } from "$lib/graphql/query";
import Spinner from "$lib/components/Spinner.svelte";
import { Plus, GitBranch, AlertCircle } from "lucide-svelte";
import Avatar from "$lib/components/Avatar.svelte";
import HealthBadge from "$lib/components/HealthBadge.svelte";
import { newRepoHref, repoHref, newOrgHref, orgHref, envIncidentHref, envHref } from "$lib/paths";
import { formatCompactTimestamp, formatDuration } from "$lib/timestamps";
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

let now = $state(Date.now());
const tick = setInterval(() => {
    now = Date.now();
}, 1000);
onDestroy(() => clearInterval(tick));
</script>

<svelte:head>
    <title>{orgName} – Skyr</title>
</svelte:head>

<div class="max-w-6xl mx-auto px-6 py-6">
  <h1 class="text-lg mb-4">
    <a href={orgHref(orgName)} class="text-gray-900 font-semibold hover:text-blue-600 transition-colors">{orgName}</a>
  </h1>

  {#if orgDetail.isPending}
    <Spinner />
  {:else if orgDetail.error}
    <div class="p-3 bg-red-50 border border-red-200 rounded text-xs text-red-600">
      {orgDetail.error.message}
    </div>
  {:else}
    {@const org = orgDetail.data.organization}
    {@const alerts = org.repositories.flatMap((repo) =>
      repo.environments.flatMap((env) =>
        (env.currentDeployment?.openIncidents ?? []).map((incident) => ({
          repo: repo.name,
          env: env.name,
          deploymentId: env.currentDeployment?.id ?? "",
          incident,
        })),
      ),
    )}

    {#if alerts.length > 0}
      <div class="mb-6 space-y-2">
        {#each alerts as alert (alert.incident.id)}
          <div class="p-3 bg-red-50 border border-red-200 rounded">
            <div class="flex items-start gap-2">
              <AlertCircle class="w-4 h-4 text-red-600 shrink-0 mt-0.5" />
              <div class="flex-1 min-w-0">
                <div class="flex flex-wrap items-baseline gap-x-2 gap-y-0.5 text-xs">
                  <span class="font-semibold text-red-900">
                    <a href={repoHref(orgName, alert.repo)} class="hover:underline">{alert.repo}</a>
                    <span class="text-red-400">/</span>
                    <a href={envHref(orgName, alert.repo, alert.env)} class="hover:underline">{alert.env}</a>
                  </span>
                  <a
                    href={envIncidentHref(orgName, alert.repo, alert.env, alert.incident.id)}
                    class="font-mono text-red-500 hover:text-red-700 hover:underline"
                  >{alert.incident.id.slice(-8)}</a>
                  <span class="ml-auto tabular-nums text-red-600 font-medium">
                    {formatCompactTimestamp(alert.incident.openedAt)}
                    <span class="text-red-400">·</span>
                    {formatDuration(now - new Date(alert.incident.openedAt).getTime())}
                  </span>
                </div>
                {#if alert.incident.summary}
                  <a
                    href={envIncidentHref(orgName, alert.repo, alert.env, alert.incident.id)}
                    class="block mt-1 text-xs text-red-800 hover:text-red-900 break-words line-clamp-2 whitespace-pre-line"
                  >
                    {alert.incident.summary}
                  </a>
                {/if}
              </div>
            </div>
          </div>
        {/each}
      </div>
    {/if}

    <div class="grid grid-cols-1 md:grid-cols-2 gap-8 items-start">
    <div>
      <div class="flex items-center justify-between mb-2">
        <h2 class="text-xs font-semibold text-gray-700">Repositories</h2>
        <a
          href={newRepoHref(orgName)}
          class="inline-flex items-center gap-1.5 px-2.5 py-1 text-xs font-medium text-gray-700 bg-white border border-gray-200 rounded hover:border-gray-300 hover:text-gray-900 transition-colors"
        >
          <Plus class="w-3.5 h-3.5" />
          New repository
        </a>
      </div>

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
              <div class="flex flex-wrap items-center justify-between gap-x-3 gap-y-1.5">
                <h3 class="text-xs font-medium text-gray-900 group-hover:text-blue-600">{repo.name}</h3>
                {#if repo.environments.length > 0}
                  <div class="flex flex-wrap items-center gap-1">
                    <GitBranch class="w-3.5 h-3.5 text-gray-400" />
                    {#if repo.environments.length > 3}
                      <span
                        title={repo.environments.map((e) => e.name).join(", ")}
                        class="text-xs text-gray-500"
                      >{repo.environments.length}</span>
                    {:else}
                      {#each repo.environments as env}
                        {@const display = env.name.length > 16
                          ? `${env.name.slice(0, 6)}…${env.name.slice(-8)}`
                          : env.name}
                        <span
                          title={env.name}
                          class="-my-0.5 inline-flex items-center gap-1 px-1.5 py-0.5 text-xs bg-gray-100 rounded text-gray-600 whitespace-nowrap"
                        >
                          {#if env.currentDeployment?.status}
                            <HealthBadge
                              health={env.currentDeployment.status.health}
                              openIncidentCount={env.currentDeployment.status.openIncidentCount}
                              size="small"
                              showLabel={false}
                            />
                          {/if}
                          {display}
                        </span>
                      {/each}
                    {/if}
                  </div>
                {/if}
              </div>
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
          <div class="flex items-center gap-2 px-4 py-2">
            <Avatar username={member.username} fullname={member.fullname} size="sm" />
            {#if member.fullname}
              <span class="text-xs font-semibold text-gray-900">{member.fullname}</span>
              <span class="text-xs text-gray-400">@{member.username}</span>
            {:else}
              <span class="text-xs font-semibold text-gray-900">@{member.username}</span>
            {/if}
          </div>
        {:else}
          <div class="px-4 py-2 text-xs text-gray-400">No members</div>
        {/each}
      </div>
    </div>
    </div>
  {/if}
</div>
