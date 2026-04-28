<script lang="ts">
import { goto } from "$app/navigation";
import { page } from "$app/stores";
import { copyText } from "$lib/clipboard";
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
    envIncidentsHref,
    envLogsHref,
    resourcesHref,
} from "$lib/paths";
import { user } from "$lib/stores/auth";
import {
    AlignJustify,
    AlertTriangle,
    Archive,
    Box,
    Check,
    ChevronDown,
    Copy,
    Download,
    Folder,
    GitBranch,
    Power,
    RefreshCw,
} from "lucide-svelte";

let { children } = $props();

let cloneDropdownOpen = $state(false);
let cloneUrl = $derived(
    `${$user?.username ?? "git"}@${$page.url.hostname}:${$page.params.org}/${$page.params.repo}`,
);
let copied = $state(false);

function copyCloneUrl() {
    copyText(`git clone -o skyr ${cloneUrl}`);
    copied = true;
    setTimeout(() => (copied = false), 2000);
}

let tearDownConfirmOpen = $state(false);
let tearDownConfirmInput = $state("");
let tearDownError = $state<string | null>(null);

const tearDown = graphqlMutation(TearDownEnvironmentDocument, {
    onSuccess: () => {
        tearDownConfirmOpen = false;
        tearDownConfirmInput = "";
        tearDownError = null;
        envDetail.refetch();
    },
    onError: (e) => {
        tearDownError = e.message;
    },
});

function handleClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (!target.closest(".clone-dropdown")) {
        cloneDropdownOpen = false;
    }
    if (!target.closest(".teardown-dropdown")) {
        tearDownConfirmOpen = false;
        tearDownConfirmInput = "";
        tearDownError = null;
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
let incidentsPath = $derived(envIncidentsHref(orgName, repoName, envName));
let activeTab = $derived(
    currentPath.startsWith(deploymentsPath)
        ? "deployments"
        : currentPath.startsWith(resPath)
          ? "resources"
          : currentPath.startsWith(artifactsPath)
            ? "artifacts"
            : currentPath.startsWith(logsPath)
              ? "logs"
              : currentPath.startsWith(incidentsPath)
                ? "incidents"
                : "files",
);
let openIncidentCount = $derived((env?.incidents ?? []).filter((i) => i.closedAt == null).length);

function truncateMid(name: string): string {
    return name.length > 40 ? `${name.slice(0, 16)}…${name.slice(-16)}` : name;
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
                  : activeTab === "incidents"
                    ? envIncidentsHref(orgName, repoName, newEnv)
                    : envHref(orgName, repoName, newEnv);
    goto(tabHref);
}
</script>

<svelte:window onclick={handleClickOutside} />

<div>
  <nav class="mb-3 flex items-center">
    <div class="inline-flex items-center relative">
      <GitBranch class="w-3.5 h-3.5 text-gray-400 absolute left-2 pointer-events-none" />
      <select
        class="appearance-none bg-white border border-gray-200 rounded pl-7 py-1 pr-7 text-xs text-gray-700 font-medium cursor-pointer hover:border-gray-300 transition-colors focus:outline-none focus:border-blue-500"
        value={envName}
        onchange={(e) => switchEnv(e.currentTarget.value)}
      >
        <option value={envName} title={envName}>{truncateMid(envName)}</option>
        {#each siblingEnvs.filter((n) => n !== envName) as name}
          <option value={name} title={name}>{truncateMid(name)}</option>
        {/each}
      </select>
      <ChevronDown class="w-3 h-3 text-gray-400 absolute right-2 pointer-events-none" />
    </div>

    <div class="teardown-dropdown relative inline-block ml-auto">
      <button
        class="inline-flex items-center gap-1.5 bg-white border border-gray-200 rounded px-2.5 py-1 text-xs text-gray-700 font-medium cursor-pointer hover:border-red-300 hover:text-red-600 transition-colors focus:outline-none focus:border-red-500"
        onclick={() => {
            tearDownConfirmOpen = !tearDownConfirmOpen;
            tearDownConfirmInput = "";
            tearDownError = null;
        }}
      >
        <Power class="w-3.5 h-3.5" />
        Tear down
      </button>

      {#if tearDownConfirmOpen}
        <div
          class="absolute right-0 mt-1 z-10 bg-white border border-gray-200 rounded-lg shadow-lg p-4 w-80"
        >
          <p class="text-sm font-medium text-gray-700 mb-2">Tear down environment?</p>
          <p class="text-sm text-gray-500 mb-3">
            This will delete all {env?.resources.length ?? 0} resource{env?.resources.length === 1 ? "" : "s"} currently living in this environment.
          </p>
          <label for="teardown-confirm" class="block text-sm text-gray-600 mb-1">
            Type <code class="bg-gray-100 px-1 py-0.5 rounded text-xs select-all">{env?.qid}</code> to confirm
          </label>
          <input
            id="teardown-confirm"
            type="text"
            class="w-full text-sm border border-gray-200 rounded-lg px-2.5 py-1.5 mb-3 focus:outline-none focus:border-red-500"
            bind:value={tearDownConfirmInput}
            placeholder={env?.qid}
          />
          {#if tearDownError}
            <div class="mb-3 p-2 bg-red-50 border border-red-200 rounded text-sm text-red-600">
              {tearDownError}
            </div>
          {/if}
          <div class="flex gap-2 justify-end">
            <button
              class="px-3 py-1.5 text-sm rounded-lg border border-gray-200 text-gray-600 hover:border-gray-400 transition-colors cursor-pointer"
              onclick={() => {
                  tearDownConfirmOpen = false;
                  tearDownConfirmInput = "";
                  tearDownError = null;
              }}
            >
              Cancel
            </button>
            <button
              class="px-3 py-1.5 text-sm rounded-lg bg-red-600 text-white hover:bg-red-700 transition-colors cursor-pointer disabled:opacity-50"
              disabled={tearDown.isPending || tearDownConfirmInput !== env?.qid}
              onclick={() => tearDown.mutate({ org: orgName, repo: repoName, env: envName })}
            >
              {tearDown.isPending ? "Tearing down..." : "Confirm tear down"}
            </button>
          </div>
        </div>
      {/if}
    </div>

    <div class="clone-dropdown relative inline-block ml-2">
      <button
        class="inline-flex items-center gap-1.5 bg-white border border-gray-200 rounded px-2.5 py-1 text-xs text-gray-700 font-medium cursor-pointer hover:border-gray-300 hover:text-gray-900 transition-colors focus:outline-none focus:border-blue-500"
        onclick={() => (cloneDropdownOpen = !cloneDropdownOpen)}
      >
        <Download class="w-3.5 h-3.5" />
        Clone
        <ChevronDown class="w-3 h-3 text-gray-400" />
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
              git clone -o skyr {cloneUrl}
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
    <div class="relative mb-3 -mx-6">
      <div class="absolute bottom-0 left-0 right-0 h-px bg-gray-200"></div>
      <div class="flex gap-1 px-6 overflow-x-auto relative">
        <a
          href={envBase}
          class="inline-flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium whitespace-nowrap transition-colors border-b-2 {activeTab ===
          'files'
            ? 'border-blue-500 text-blue-600'
            : 'border-transparent text-gray-500 hover:text-gray-700'}"
        >
          <Folder class="w-4 h-4" />
          Files
        </a>
        <a
          href={deploymentsPath}
          class="inline-flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium whitespace-nowrap transition-colors border-b-2 {activeTab ===
          'deployments'
            ? 'border-blue-500 text-blue-600'
            : 'border-transparent text-gray-500 hover:text-gray-700'}"
        >
          <RefreshCw class="w-4 h-4" />
          Deployments <span class="text-gray-400 ml-1"
            >({env.deployments.length})</span
          >
        </a>
        <a
          href={resPath}
          class="inline-flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium whitespace-nowrap transition-colors border-b-2 {activeTab ===
          'resources'
            ? 'border-blue-500 text-blue-600'
            : 'border-transparent text-gray-500 hover:text-gray-700'}"
        >
          <Box class="w-4 h-4" />
          Resources <span class="text-gray-400 ml-1"
            >({env.resources.length})</span
          >
        </a>
        <a
          href={artifactsPath}
          class="inline-flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium whitespace-nowrap transition-colors border-b-2 {activeTab ===
          'artifacts'
            ? 'border-blue-500 text-blue-600'
            : 'border-transparent text-gray-500 hover:text-gray-700'}"
        >
          <Archive class="w-4 h-4" />
          Artifacts {#if env.artifacts.length > 0}<span class="text-gray-400 ml-1"
            >({env.artifacts.length})</span
          >{/if}
        </a>
        <a
          href={logsPath}
          class="inline-flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium whitespace-nowrap transition-colors border-b-2 {activeTab ===
          'logs'
            ? 'border-blue-500 text-blue-600'
            : 'border-transparent text-gray-500 hover:text-gray-700'}"
        >
          <AlignJustify class="w-4 h-4" />
          Logs
        </a>
        <a
          href={incidentsPath}
          class="inline-flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium whitespace-nowrap transition-colors border-b-2 {activeTab ===
          'incidents'
            ? 'border-blue-500 text-blue-600'
            : 'border-transparent text-gray-500 hover:text-gray-700'}"
        >
          <AlertTriangle class="w-4 h-4" />
          Incidents {#if openIncidentCount > 0}<span class="text-red-500 ml-1"
            >({openIncidentCount})</span
          >{/if}
        </a>
      </div>
    </div>

    {@render children()}
  {/if}
</div>
