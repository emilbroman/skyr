<script lang="ts">
import { page } from "$app/stores";
import { Box, MoreVertical } from "lucide-svelte";
import DeploymentStateBadge from "$lib/components/DeploymentState.svelte";
import HealthBadge from "$lib/components/HealthBadge.svelte";
import {
    CreateDeploymentDocument,
    DeploymentState,
    EnvironmentDetailDocument,
    ResourceMarker,
} from "$lib/graphql/generated";
import { graphqlMutation, graphqlQuery } from "$lib/graphql/query";
import { decodeSegment, deploymentHref } from "$lib/paths";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let envName = $derived(decodeSegment($page.params.env ?? ""));

const envDetail = graphqlQuery(() => ({
    document: EnvironmentDetailDocument,
    variables: { org: orgName, repo: repoName, env: envName },
    refetchInterval: 10_000,
}));

let env = $derived(envDetail.data?.organization.repository.environment ?? null);

type SortColumn = "id" | "deployedAt" | "state" | "resources";
type SortDirection = "asc" | "desc";

let sortColumn: SortColumn = $state("deployedAt");
let sortDirection: SortDirection = $state("desc");

function toggleSort(column: SortColumn) {
    if (sortColumn === column) {
        sortDirection = sortDirection === "asc" ? "desc" : "asc";
    } else {
        sortColumn = column;
        sortDirection = "asc";
    }
}

let sortedDeployments = $derived.by(() => {
    if (!env) return [];
    const deps = [...env.deployments];
    const dir = sortDirection === "asc" ? 1 : -1;
    deps.sort((a, b) => {
        switch (sortColumn) {
            case "id":
                return dir * a.commit.hash.localeCompare(b.commit.hash);
            case "deployedAt":
                return dir * a.createdAt.localeCompare(b.createdAt);
            case "state":
                return dir * a.state.localeCompare(b.state);
            case "resources":
                return dir * (a.resources.length - b.resources.length);
            default:
                return 0;
        }
    });
    return deps;
});

function sortIndicator(column: SortColumn): string {
    if (sortColumn !== column) return "";
    return sortDirection === "asc" ? " \u25B2" : " \u25BC";
}

let pendingCommit = $state<string | null>(null);
let createDeploymentError = $state<string | null>(null);

const createDeployment = graphqlMutation(CreateDeploymentDocument, {
    onSuccess: () => {
        pendingCommit = null;
        createDeploymentError = null;
        envDetail.refetch();
    },
    onError: (e) => {
        pendingCommit = null;
        createDeploymentError = e.message;
    },
});

function onDeploy(commitHash: string) {
    pendingCommit = commitHash;
    createDeploymentError = null;
    createDeployment.mutate({
        org: orgName,
        repo: repoName,
        env: envName,
        commitHash,
    });
}

let openMenuId = $state<string | null>(null);

function toggleMenu(id: string) {
    openMenuId = openMenuId === id ? null : id;
}

function onWindowClick(event: MouseEvent) {
    if (openMenuId === null) return;
    const target = event.target as Element | null;
    if (target?.closest(`[data-menu-root="${openMenuId}"]`)) return;
    openMenuId = null;
}
</script>

<svelte:window onclick={onWindowClick} />

<svelte:head>
    <title>Deployments · {orgName}/{repoName} ({envName}) – Skyr</title>
</svelte:head>

{#if createDeploymentError}
  <div class="mb-3 p-2 bg-red-50 border border-red-200 rounded text-xs text-red-600">
    {createDeploymentError}
  </div>
{/if}

{#if env}
  {#if env.deployments.length === 0}
    <div class="text-center py-12 border border-dashed border-gray-200 rounded">
      <p class="text-xs text-gray-500">No deployments.</p>
    </div>
  {:else}
    <div class="hidden md:block bg-white border border-gray-200 rounded overflow-hidden">
    <table class="w-full text-left text-xs">
      <thead>
        <tr class="border-b border-gray-200 text-gray-500 bg-gray-50">
          <th class="py-2 pl-4 pr-4 font-semibold text-xs text-gray-700"></th>
          <th class="py-2 pr-4 font-medium">
            <button
              class="font-semibold text-xs text-gray-700 hover:text-gray-900 transition-colors cursor-pointer"
              onclick={() => toggleSort("id")}
            >
              Deployment{sortIndicator("id")}
            </button>
          </th>
          <th class="py-2 pr-4 font-medium">
            <button
              class="font-semibold text-xs text-gray-700 hover:text-gray-900 transition-colors cursor-pointer"
              onclick={() => toggleSort("deployedAt")}
            >
              Deployed at{sortIndicator("deployedAt")}
            </button>
          </th>
          <th class="py-2 pr-4 font-medium">
            <button
              class="font-semibold text-xs text-gray-700 hover:text-gray-900 transition-colors cursor-pointer"
              onclick={() => toggleSort("state")}
            >
              State{sortIndicator("state")}
            </button>
          </th>
          <th class="py-2 pr-4 font-medium">
            <button
              class="font-semibold text-xs text-gray-700 hover:text-gray-900 transition-colors cursor-pointer"
              onclick={() => toggleSort("resources")}
            >
              Resources{sortIndicator("resources")}
            </button>
          </th>
          <th class="py-2 pr-4 font-medium"></th>
        </tr>
      </thead>
      <tbody class="divide-y divide-gray-100">
        {#each sortedDeployments as deployment}
          <tr class="hover:bg-gray-50">
            <td class="py-2 pl-4 pr-4">
              {#if deployment.state === DeploymentState.Down}
                <span class="inline-block w-2 h-2 rounded-full bg-gray-400" aria-label="Down"></span>
              {:else}
                <HealthBadge
                  health={deployment.status.health}
                  openIncidentCount={deployment.status.openIncidentCount}
                  showLabel={false}
                />
              {/if}
            </td>
            <td class="py-2 pr-4">
              <a
                href={deploymentHref(orgName, repoName, envName, deployment.id)}
                class="group flex items-baseline gap-3 max-w-xl"
              >
                <span class="font-mono text-gray-500 shrink-0 group-hover:text-gray-700">{deployment.shortId}</span>
                <span class="text-gray-800 group-hover:text-blue-600 truncate min-w-0">
                  {deployment.commit.message.split("\n")[0]}
                </span>
              </a>
            </td>
            <td class="py-2 pr-4 text-gray-500">
              {new Date(deployment.createdAt).toLocaleString()}
            </td>
            <td class="py-2 pr-4">
              <DeploymentStateBadge state={deployment.state} bootstrapped={deployment.bootstrapped} volatile={deployment.resources.some((r) => r.markers.includes(ResourceMarker.Volatile))} size="small" />
            </td>
            <td class="py-2 pr-4">
              {#if deployment.resources.length > 0}
                <span
                  class="inline-flex items-center gap-1 rounded text-xs font-medium border bg-gray-100 border-gray-300 text-gray-500 px-1.5 py-px"
                  title="{deployment.resources.length} resource{deployment.resources.length === 1 ? '' : 's'}"
                >
                  <Box class="w-2.5 h-2.5" />
                  {deployment.resources.length}
                </span>
              {/if}
            </td>
            <td class="py-2 pr-4 text-right">
              {#if deployment.state !== DeploymentState.Desired}
                <button
                  type="button"
                  onclick={() => onDeploy(deployment.commit.hash)}
                  disabled={createDeployment.isPending && pendingCommit === deployment.commit.hash}
                  class="inline-flex items-center gap-1.5 -my-1 bg-white border border-gray-200 rounded px-2.5 py-1 text-xs text-gray-700 font-medium cursor-pointer hover:border-gray-300 hover:text-gray-900 transition-colors focus:outline-none focus:border-blue-500 disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  {createDeployment.isPending && pendingCommit === deployment.commit.hash
                    ? "Deploying..."
                    : "Deploy"}
                </button>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
    </div>

    <div class="md:hidden space-y-2">
      {#each sortedDeployments as deployment}
        <div class="relative bg-white border border-gray-200 rounded-lg p-3">
          <div class="flex items-center justify-between text-xs text-gray-500">
            <span class="font-mono">{deployment.shortId}</span>
            <span>{new Date(deployment.createdAt).toLocaleString()}</span>
          </div>
          <div class="mt-1 truncate">
            <a
              href={deploymentHref(orgName, repoName, envName, deployment.id)}
              class="text-blue-600 hover:text-blue-500"
            >
              {deployment.commit.message.split("\n")[0]}
            </a>
          </div>
          <div class="mt-2 flex items-center gap-3">
            <DeploymentStateBadge state={deployment.state} bootstrapped={deployment.bootstrapped} volatile={deployment.resources.some((r) => r.markers.includes(ResourceMarker.Volatile))} size="small" />
            {#if deployment.state !== DeploymentState.Down}
              <HealthBadge health={deployment.status.health} openIncidentCount={deployment.status.openIncidentCount} size="small" />
            {/if}
            {#if deployment.resources.length > 0}
              <span
                class="inline-flex items-center gap-1 rounded text-xs font-medium border bg-gray-100 border-gray-300 text-gray-500 px-1.5 py-px"
                title="{deployment.resources.length} resource{deployment.resources.length === 1 ? '' : 's'}"
              >
                <Box class="w-2.5 h-2.5" />
                {deployment.resources.length}
              </span>
            {/if}
            {#if deployment.state !== DeploymentState.Desired}
              <div class="ml-auto -my-1" data-menu-root={deployment.id}>
                <button
                  type="button"
                  onclick={() => toggleMenu(deployment.id)}
                  class="p-1 text-gray-500 hover:text-gray-800 hover:bg-gray-100 rounded cursor-pointer"
                  aria-label="Open menu"
                >
                  <MoreVertical class="w-5 h-5" />
                </button>
                {#if openMenuId === deployment.id}
                  <div class="absolute right-0 top-full mt-1 z-10 bg-white border border-gray-200 rounded shadow-md min-w-[10rem] py-1">
                    <button
                      type="button"
                      onclick={() => {
                        openMenuId = null;
                        onDeploy(deployment.commit.hash);
                      }}
                      disabled={createDeployment.isPending && pendingCommit === deployment.commit.hash}
                      class="w-full text-left px-3 py-2 text-sm hover:bg-gray-50 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
                    >
                      {createDeployment.isPending && pendingCommit === deployment.commit.hash
                        ? "Deploying..."
                        : "Deploy"}
                    </button>
                  </div>
                {/if}
              </div>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {/if}
{/if}
