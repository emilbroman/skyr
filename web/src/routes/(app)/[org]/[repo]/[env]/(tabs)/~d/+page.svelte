<script lang="ts">
import { page } from "$app/stores";
import DeploymentStateBadge from "$lib/components/DeploymentState.svelte";
import { EnvironmentDetailDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
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
</script>

{#if env}
  {#if env.deployments.length === 0}
    <p class="text-gray-500">No deployments.</p>
  {:else}
    <div class="bg-white border border-gray-200 rounded-lg overflow-hidden">
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
                href={deploymentHref(orgName, repoName, envName, deployment.commit.hash)}
                class="text-orange-600 hover:text-orange-500"
              >
                {deployment.commit.message.split("\n")[0]}
              </a>
            </td>
            <td class="py-3 pr-4 text-gray-500">
              {new Date(deployment.createdAt).toLocaleString()}
            </td>
            <td class="py-3 pr-4">
              <DeploymentStateBadge state={deployment.state} size="small" />
            </td>
            <td class="py-3 pr-4 text-gray-500">
              {deployment.resources.length}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
    </div>
  {/if}
{/if}
