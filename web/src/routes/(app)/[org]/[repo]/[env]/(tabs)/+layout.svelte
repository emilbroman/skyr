<script lang="ts">
import { goto } from "$app/navigation";
import { page } from "$app/stores";
import { EnvironmentDetailDocument, RepositoryDetailDocument } from "$lib/graphql/generated";
import Spinner from "$lib/components/Spinner.svelte";
import { graphqlQuery } from "$lib/graphql/query";
import { decodeSegment, envDeploymentsHref, envHref, envLogsHref, resourcesHref } from "$lib/paths";
import { user } from "$lib/stores/auth";

let { children } = $props();

let cloneDropdownOpen = $state(false);
let cloneUrl = $derived(
    `${$user?.username ?? "git"}@${$page.url.hostname}:${$page.params.org}/${$page.params.repo}`,
);
let copied = $state(false);

function copyCloneUrl() {
    navigator.clipboard.writeText(`git clone ${cloneUrl}`);
    copied = true;
    setTimeout(() => (copied = false), 2000);
}

function handleCloneClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (!target.closest(".clone-dropdown")) {
        cloneDropdownOpen = false;
    }
}

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let envName = $derived(decodeSegment($page.params.env ?? ""));

const envDetail = graphqlQuery(() => ({
    document: EnvironmentDetailDocument,
    variables: { org: orgName, repo: repoName, env: envName },
    refetchInterval: 10_000,
}));

const repoDetail = graphqlQuery(() => ({
    document: RepositoryDetailDocument,
    variables: { org: orgName, repo: repoName },
}));

let env = $derived(envDetail.data?.organization.repository.environment ?? null);
let siblingEnvs = $derived(
    repoDetail.data?.organization.repository.environments.map((e) => e.name) ?? [],
);

let currentPath = $derived($page.url.pathname);
let envBase = $derived(envHref(orgName, repoName, envName));
let deploymentsPath = $derived(envDeploymentsHref(orgName, repoName, envName));
let resPath = $derived(resourcesHref(orgName, repoName, envName));
let logsPath = $derived(envLogsHref(orgName, repoName, envName));
let activeTab = $derived(
    currentPath.startsWith(deploymentsPath)
        ? "deployments"
        : currentPath.startsWith(resPath)
          ? "resources"
          : currentPath.startsWith(logsPath)
            ? "logs"
            : "files",
);

function switchEnv(newEnv: string) {
    const tabHref =
        activeTab === "deployments"
            ? envDeploymentsHref(orgName, repoName, newEnv)
            : activeTab === "resources"
              ? resourcesHref(orgName, repoName, newEnv)
              : activeTab === "logs"
                ? envLogsHref(orgName, repoName, newEnv)
                : envHref(orgName, repoName, newEnv);
    goto(tabHref);
}
</script>

<svelte:window onclick={handleCloneClickOutside} />

<div>
  <nav class="mb-2 flex items-center">
    <div class="inline-flex items-center relative">
      <select
        class="appearance-none bg-white border border-gray-200 rounded-lg px-3 py-1.5 pr-8 text-gray-600 font-medium cursor-pointer hover:border-gray-400 transition-colors focus:outline-none focus:border-orange-500"
        value={envName}
        onchange={(e) => switchEnv(e.currentTarget.value)}
      >
        <option value={envName}>{envName}</option>
        {#each siblingEnvs.filter((n) => n !== envName) as name}
          <option value={name}>{name}</option>
        {/each}
      </select>
      <svg
        class="w-3.5 h-3.5 text-gray-400 absolute right-2.5 pointer-events-none"
        fill="none"
        viewBox="0 0 24 24"
        stroke="currentColor"
      >
        <path
          stroke-linecap="round"
          stroke-linejoin="round"
          stroke-width="2"
          d="M19 9l-7 7-7-7"
        />
      </svg>
    </div>

    <div class="clone-dropdown relative inline-block ml-2">
      <button
        class="inline-flex items-center gap-1.5 bg-white border border-gray-200 rounded-lg px-3 py-1.5 text-gray-600 font-medium cursor-pointer hover:border-gray-400 transition-colors focus:outline-none focus:border-orange-500"
        onclick={() => (cloneDropdownOpen = !cloneDropdownOpen)}
      >
        <svg class="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4"
          />
        </svg>
        Clone
        <svg class="w-3.5 h-3.5 text-gray-400" fill="none" viewBox="0 0 24 24" stroke="currentColor">
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M19 9l-7 7-7-7"
          />
        </svg>
      </button>

      {#if cloneDropdownOpen}
        <div
          class="absolute left-0 mt-1 z-10 bg-white border border-gray-200 rounded-lg shadow-lg p-4 w-80"
        >
          <p class="text-sm font-medium text-gray-700 mb-2">Clone with SSH</p>
          <div class="flex items-center gap-2">
            <code
              class="flex-1 text-xs bg-gray-50 border border-gray-200 rounded px-2 py-1.5 text-gray-800 overflow-x-auto whitespace-nowrap"
            >
              git clone {cloneUrl}
            </code>
            <button
              class="shrink-0 p-1.5 rounded hover:bg-gray-100 transition-colors text-gray-500 hover:text-gray-700"
              title="Copy to clipboard"
              onclick={copyCloneUrl}
            >
              {#if copied}
                <svg class="w-4 h-4 text-green-500" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" />
                </svg>
              {:else}
                <svg class="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                  <path
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    stroke-width="2"
                    d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z"
                  />
                </svg>
              {/if}
            </button>
          </div>
        </div>
      {/if}
    </div>
  </nav>

  {#if envDetail.isPending}
    <Spinner />
  {:else if envDetail.error}
    <div class="p-4 bg-red-50 border border-red-200 rounded text-red-600">
      {envDetail.error.message}
    </div>
  {:else if env}
    <!-- Tabs -->
    <div class="flex gap-1 border-b border-gray-200 mb-3 overflow-x-auto overflow-y-hidden">
      <a
        href={envBase}
        class="px-3 py-2 whitespace-nowrap transition-colors border-b-3 -mb-px {activeTab ===
        'files'
          ? 'border-orange-500 text-gray-900'
          : 'border-transparent text-gray-500 hover:text-gray-800'}"
      >
        Files
      </a>
      <a
        href={deploymentsPath}
        class="px-3 py-2 whitespace-nowrap transition-colors border-b-3 -mb-px {activeTab ===
        'deployments'
          ? 'border-orange-500 text-gray-900'
          : 'border-transparent text-gray-500 hover:text-gray-800'}"
      >
        Deployments <span class="text-gray-400 ml-1"
          >({env.deployments.length})</span
        >
      </a>
      <a
        href={resPath}
        class="px-3 py-2 whitespace-nowrap transition-colors border-b-3 -mb-px {activeTab ===
        'resources'
          ? 'border-orange-500 text-gray-900'
          : 'border-transparent text-gray-500 hover:text-gray-800'}"
      >
        Resources <span class="text-gray-400 ml-1"
          >({env.resources.length})</span
        >
      </a>
      <a
        href={logsPath}
        class="px-3 py-2 whitespace-nowrap transition-colors border-b-3 -mb-px {activeTab ===
        'logs'
          ? 'border-orange-500 text-gray-900'
          : 'border-transparent text-gray-500 hover:text-gray-800'}"
      >
        Logs
      </a>
    </div>

    {@render children()}
  {/if}
</div>
