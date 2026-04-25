<script lang="ts">
import { page } from "$app/stores";
import { MoreVertical } from "lucide-svelte";
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

type SortColumn = "id" | "commit" | "deployedAt" | "state" | "resources";
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
            case "commit":
                return dir * a.commit.message.localeCompare(b.commit.message);
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
    if (target && target.closest(`[data-menu-root="${openMenuId}"]`)) return;
    openMenuId = null;
}
</script>

<svelte:window onclick={onWindowClick} />

<svelte:head>
    <title>Deployments · {orgName}/{repoName} ({envName}) – Skyr</title>
</svelte:head>

{#if createDeploymentError}
  <div class="mb-4 p-3 bg-red-50 border border-red-200 rounded text-red-600">
    {createDeploymentError}
  </div>
{/if}

{#if env}
  {#if env.deployments.length === 0}
    <p class="text-gray-500">No deployments.</p>
  {:else}
    <div class="hidden md:block bg-white border border-gray-200 rounded-lg overflow-hidden">
    <table class="w-full text-left">
      <thead>
        <tr class="border-b border-gray-200 text-gray-500">
          <th class="pb-3 pt-3 pl-4 pr-4 font-medium">
            <button
              class="hover:text-gray-800 transition-colors cursor-pointer"
              onclick={() => toggleSort("id")}
            >
              ID{sortIndicator("id")}
            </button>
          </th>
          <th class="pb-3 pt-3 pr-4 font-medium">
            <button
              class="hover:text-gray-800 transition-colors cursor-pointer"
              onclick={() => toggleSort("commit")}
            >
              Commit{sortIndicator("commit")}
            </button>
          </th>
          <th class="pb-3 pt-3 pr-4 font-medium">
            <button
              class="hover:text-gray-800 transition-colors cursor-pointer"
              onclick={() => toggleSort("deployedAt")}
            >
              Deployed at{sortIndicator("deployedAt")}
            </button>
          </th>
          <th class="pb-3 pt-3 pr-4 font-medium">
            <button
              class="hover:text-gray-800 transition-colors cursor-pointer"
              onclick={() => toggleSort("state")}
            >
              State{sortIndicator("state")}
            </button>
          </th>
          <th class="pb-3 pt-3 pr-4 font-medium">
            <button
              class="hover:text-gray-800 transition-colors cursor-pointer"
              onclick={() => toggleSort("resources")}
            >
              Resources{sortIndicator("resources")}
            </button>
          </th>
          <th class="pb-3 pt-3 pr-4 font-medium"></th>
        </tr>
      </thead>
      <tbody>
        {#each sortedDeployments as deployment}
          <tr class="border-b border-gray-200 hover:bg-gray-50">
            <td class="py-3 pl-4 pr-4 font-mono text-xs text-gray-500">
              {deployment.commit.hash.substring(0, 8)}
            </td>
            <td class="py-3 pr-4 truncate max-w-md">
              <a
                href={deploymentHref(orgName, repoName, envName, `${deployment.commit.hash}.${deployment.nonce}`)}
                class="text-orange-600 hover:text-orange-500"
              >
                {deployment.commit.message.split("\n")[0]}
              </a>
            </td>
            <td class="py-3 pr-4 text-gray-500">
              {new Date(deployment.createdAt).toLocaleString()}
            </td>
            <td class="py-3 pr-4">
              <div class="flex items-center gap-1.5">
                <DeploymentStateBadge state={deployment.state} bootstrapped={deployment.bootstrapped} failures={deployment.failures} volatile={deployment.resources.some((r) => r.markers.includes(ResourceMarker.Volatile))} size="small" />
                <HealthBadge health={deployment.status.health} openIncidentCount={deployment.status.openIncidentCount} worstOpenCategory={deployment.status.worstOpenCategory} size="small" showLabel={false} />
              </div>
            </td>
            <td class="py-3 pr-4 text-gray-500">
              {deployment.resources.length}
            </td>
            <td class="py-3 pr-4 text-right">
              {#if deployment.state !== DeploymentState.Desired}
                <button
                  type="button"
                  onclick={() => onDeploy(deployment.commit.hash)}
                  disabled={createDeployment.isPending && pendingCommit === deployment.commit.hash}
                  class="px-2 py-1 text-xs bg-orange-600 hover:bg-orange-500 text-gray-900 rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
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

    <div class="md:hidden space-y-3">
      {#each sortedDeployments as deployment}
        <div class="relative bg-white border border-gray-200 rounded-lg p-4">
          <div class="flex items-center justify-between text-xs text-gray-500">
            <span class="font-mono">{deployment.commit.hash.substring(0, 8)}</span>
            <span>{new Date(deployment.createdAt).toLocaleString()}</span>
          </div>
          <div class="mt-2 truncate">
            <a
              href={deploymentHref(orgName, repoName, envName, `${deployment.commit.hash}.${deployment.nonce}`)}
              class="text-orange-600 hover:text-orange-500"
            >
              {deployment.commit.message.split("\n")[0]}
            </a>
          </div>
          <div class="mt-3 flex items-center gap-3">
            <DeploymentStateBadge state={deployment.state} bootstrapped={deployment.bootstrapped} failures={deployment.failures} volatile={deployment.resources.some((r) => r.markers.includes(ResourceMarker.Volatile))} size="small" />
            <HealthBadge health={deployment.status.health} openIncidentCount={deployment.status.openIncidentCount} worstOpenCategory={deployment.status.worstOpenCategory} size="small" showLabel={false} />
            <span class="text-sm text-gray-500">{deployment.resources.length} resources</span>
            {#if deployment.state !== DeploymentState.Desired}
              <div class="ml-auto" data-menu-root={deployment.id}>
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
