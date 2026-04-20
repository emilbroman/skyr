<script lang="ts">
import { goto } from "$app/navigation";
import { page } from "$app/stores";
import {
    EnvironmentDetailDocument,
    RepositoryDetailDocument,
    TearDownEnvironmentDocument,
} from "$lib/graphql/generated";
import Spinner from "$lib/components/Spinner.svelte";
import { graphqlMutation, graphqlQuery } from "$lib/graphql/query";
import {
    decodeSegment,
    envArtifactsHref,
    envDeploymentsHref,
    envHref,
    envLogsHref,
    repoHref,
    resourcesHref,
} from "$lib/paths";
import { user } from "$lib/stores/auth";
import {
    AlignJustify,
    Archive,
    Box,
    Check,
    ChevronDown,
    Copy,
    Download,
    Folder,
    GitBranch,
    RefreshCw,
    Trash2,
} from "lucide-svelte";

let { children } = $props();

let cloneDropdownOpen = $state(false);
let cloneUrl = $derived(
    `${$user?.username ?? "git"}@${$page.url.hostname}:${$page.params.org}/${$page.params.repo}`,
);
let copied = $state(false);

let tearDownConfirmOpen = $state(false);
let tearDownError = $state<string | null>(null);

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
let artifactsPath = $derived(envArtifactsHref(orgName, repoName, envName));
let logsPath = $derived(envLogsHref(orgName, repoName, envName));
let activeTab = $derived(
    currentPath.startsWith(deploymentsPath)
        ? "deployments"
        : currentPath.startsWith(resPath)
          ? "resources"
          : currentPath.startsWith(artifactsPath)
            ? "artifacts"
            : currentPath.startsWith(logsPath)
              ? "logs"
              : "files",
);

const tearDown = graphqlMutation(TearDownEnvironmentDocument, {
    onSuccess: () => {
        tearDownConfirmOpen = false;
        tearDownError = null;
        goto(repoHref(orgName, repoName));
    },
    onError: (e) => {
        tearDownError = e.message;
    },
});

function confirmTearDown() {
    tearDownError = null;
    tearDown.mutate({ org: orgName, repo: repoName, env: envName });
}

function switchEnv(newEnv: string) {
    const tabHref =
        activeTab === "deployments"
            ? envDeploymentsHref(orgName, repoName, newEnv)
            : activeTab === "resources"
              ? resourcesHref(orgName, repoName, newEnv)
              : activeTab === "artifacts"
                ? envArtifactsHref(orgName, repoName, newEnv)
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

    <div class="ml-auto flex items-center gap-2">
      <div class="clone-dropdown relative inline-block">
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

      <button
        type="button"
        class="inline-flex items-center gap-1.5 bg-white border border-gray-200 rounded-lg px-3 py-1.5 text-red-600 font-medium cursor-pointer hover:border-red-400 transition-colors focus:outline-none focus:border-red-500"
        onclick={() => {
            tearDownError = null;
            tearDownConfirmOpen = true;
        }}
      >
        <Trash2 class="w-4 h-4" />
        Tear down
      </button>
    </div>
  </nav>

  {#if tearDownConfirmOpen}
    <div
      class="fixed inset-0 z-20 flex items-center justify-center bg-black/40"
      role="dialog"
      aria-modal="true"
    >
      <div class="bg-white rounded-lg shadow-xl p-6 w-full max-w-md mx-4">
        <h2 class="text-lg font-semibold text-gray-900 mb-2">Tear down environment</h2>
        <p class="text-sm text-gray-600 mb-4">
          This will mark every active deployment in
          <code class="font-mono text-gray-800">{envName}</code>
          as undesired, taking the environment down. This cannot be undone from the UI.
        </p>
        {#if tearDownError}
          <div class="mb-3 p-2 bg-red-50 border border-red-200 rounded text-sm text-red-600">
            {tearDownError}
          </div>
        {/if}
        <div class="flex justify-end gap-2">
          <button
            type="button"
            class="px-3 py-1.5 rounded border border-gray-200 text-gray-700 hover:bg-gray-50 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
            disabled={tearDown.isPending}
            onclick={() => (tearDownConfirmOpen = false)}
          >
            Cancel
          </button>
          <button
            type="button"
            class="px-3 py-1.5 rounded bg-red-600 hover:bg-red-500 text-white font-medium transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
            disabled={tearDown.isPending}
            onclick={confirmTearDown}
          >
            {tearDown.isPending ? "Tearing down..." : "Tear down"}
          </button>
        </div>
      </div>
    </div>
  {/if}

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
        href={artifactsPath}
        class="inline-flex items-center gap-1.5 px-2 py-2 whitespace-nowrap transition-colors border-b-3 -mb-px {activeTab ===
        'artifacts'
          ? 'border-orange-500 text-gray-900'
          : 'border-transparent text-gray-500 hover:text-gray-800'}"
      >
        <Archive class="w-4 h-4" />
        Artifacts {#if env.artifacts.length > 0}<span class="text-gray-400 ml-1"
          >({env.artifacts.length})</span
        >{/if}
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
