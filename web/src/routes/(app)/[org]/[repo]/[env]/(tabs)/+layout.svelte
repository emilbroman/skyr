<script lang="ts">
import { goto } from "$app/navigation";
import { page } from "$app/stores";
import { EnvironmentDetailDocument, RepositoryDetailDocument } from "$lib/graphql/generated";
import Spinner from "$lib/components/Spinner.svelte";
import { graphqlQuery } from "$lib/graphql/query";
import { decodeSegment, envDeploymentsHref, envHref, envLogsHref, resourcesHref } from "$lib/paths";
import { user } from "$lib/stores/auth";
import {
    AlignJustify,
    Box,
    Check,
    ChevronDown,
    Copy,
    Download,
    Folder,
    GitBranch,
    RefreshCw,
} from "lucide-svelte";

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
      <GitBranch class="w-4 h-4 text-gray-400 absolute left-2.5 pointer-events-none" />
      <select
        class="appearance-none bg-white border border-gray-200 rounded-lg pl-8 py-1.5 pr-8 text-gray-600 font-medium cursor-pointer hover:border-gray-400 transition-colors focus:outline-none focus:border-orange-500"
        value={envName}
        onchange={(e) => switchEnv(e.currentTarget.value)}
      >
        <option value={envName}>{envName}</option>
        {#each siblingEnvs.filter((n) => n !== envName) as name}
          <option value={name}>{name}</option>
        {/each}
      </select>
      <ChevronDown class="w-3.5 h-3.5 text-gray-400 absolute right-2.5 pointer-events-none" />
    </div>

    <div class="clone-dropdown relative inline-block ml-auto">
      <button
        class="inline-flex items-center gap-1.5 bg-white border border-gray-200 rounded-lg px-3 py-1.5 text-gray-600 font-medium cursor-pointer hover:border-gray-400 transition-colors focus:outline-none focus:border-orange-500"
        onclick={() => (cloneDropdownOpen = !cloneDropdownOpen)}
      >
        <Download class="w-4 h-4" />
        Clone
        <ChevronDown class="w-3.5 h-3.5 text-gray-400" />
      </button>

      {#if cloneDropdownOpen}
        <div
          class="absolute right-0 mt-1 z-10 bg-white border border-gray-200 rounded-lg shadow-lg p-4 w-80"
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
                <Check class="w-4 h-4 text-green-500" />
              {:else}
                <Copy class="w-4 h-4" />
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
        class="inline-flex items-center gap-1.5 px-2 py-2 whitespace-nowrap transition-colors border-b-3 -mb-px {activeTab ===
        'files'
          ? 'border-orange-500 text-gray-900'
          : 'border-transparent text-gray-500 hover:text-gray-800'}"
      >
        <Folder class="w-4 h-4" />
        Files
      </a>
      <a
        href={deploymentsPath}
        class="inline-flex items-center gap-1.5 px-2 py-2 whitespace-nowrap transition-colors border-b-3 -mb-px {activeTab ===
        'deployments'
          ? 'border-orange-500 text-gray-900'
          : 'border-transparent text-gray-500 hover:text-gray-800'}"
      >
        <RefreshCw class="w-4 h-4" />
        Deployments <span class="text-gray-400 ml-1"
          >({env.deployments.length})</span
        >
      </a>
      <a
        href={resPath}
        class="inline-flex items-center gap-1.5 px-2 py-2 whitespace-nowrap transition-colors border-b-3 -mb-px {activeTab ===
        'resources'
          ? 'border-orange-500 text-gray-900'
          : 'border-transparent text-gray-500 hover:text-gray-800'}"
      >
        <Box class="w-4 h-4" />
        Resources <span class="text-gray-400 ml-1"
          >({env.resources.length})</span
        >
      </a>
      <a
        href={logsPath}
        class="inline-flex items-center gap-1.5 px-2 py-2 whitespace-nowrap transition-colors border-b-3 -mb-px {activeTab ===
        'logs'
          ? 'border-orange-500 text-gray-900'
          : 'border-transparent text-gray-500 hover:text-gray-800'}"
      >
        <AlignJustify class="w-4 h-4" />
        Logs
      </a>
    </div>

    {@render children()}
  {/if}
</div>
