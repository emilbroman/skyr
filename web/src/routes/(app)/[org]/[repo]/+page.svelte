<script lang="ts">
import { page } from "$app/stores";
import RootTree from "$lib/components/RootTree.svelte";
import { DeploymentState, RepositoryDetailDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { deploymentHref, envHref, orgHref, resourcesHref } from "$lib/paths";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");

const repoDetail = graphqlQuery(() => ({
    document: RepositoryDetailDocument,
    variables: { org: orgName, repo: repoName },
    refetchInterval: 10_000,
}));

let repo = $derived(repoDetail.data?.organization.repository ?? null);

// Find the "main" environment (named "main" or the first one) and its desired deployment
let mainEnv = $derived(
    repo?.environments.find((e) => e.name === "main") ?? repo?.environments[0] ?? null,
);

let mainDesiredDeployment = $derived(
    mainEnv?.deployments.find(
        (d) => d.state === DeploymentState.Desired || d.state === DeploymentState.Up,
    ) ?? null,
);

type SortColumn = "name" | "deployment" | "resources";
type SortDirection = "asc" | "desc";

let sortColumn: SortColumn = $state("name");
let sortDirection: SortDirection = $state("asc");

function toggleSort(column: SortColumn) {
    if (sortColumn === column) {
        sortDirection = sortDirection === "asc" ? "desc" : "asc";
    } else {
        sortColumn = column;
        sortDirection = "asc";
    }
}

function desiredDeployment(env: {
    deployments: { state: DeploymentState; commit: { hash: string; message: string } }[];
}) {
    return (
        env.deployments.find(
            (d) => d.state === DeploymentState.Desired || d.state === DeploymentState.Up,
        ) ?? null
    );
}

let sortedEnvironments = $derived.by(() => {
    if (!repo) return [];
    const envs = [...repo.environments];
    const dir = sortDirection === "asc" ? 1 : -1;
    envs.sort((a, b) => {
        switch (sortColumn) {
            case "name":
                return dir * a.name.localeCompare(b.name);
            case "deployment": {
                const ha = desiredDeployment(a)?.commit.hash ?? "";
                const hb = desiredDeployment(b)?.commit.hash ?? "";
                return dir * ha.localeCompare(hb);
            }
            case "resources":
                return dir * (a.resources.length - b.resources.length);
            default:
                return 0;
        }
    });
    return envs;
});

function sortIndicator(column: SortColumn): string {
    if (sortColumn !== column) return "";
    return sortDirection === "asc" ? " \u25B2" : " \u25BC";
}
</script>

<div class="p-6">
  <nav class="text-sm text-gray-500 mb-4">
    <a href={orgHref(orgName)} class="hover:text-gray-300">{orgName}</a>
    <span class="mx-2">/</span>
    <span class="text-gray-300">{repoName}</span>
  </nav>

  <h1 class="text-2xl font-bold text-white mb-6">{orgName}/{repoName}</h1>

  {#if repoDetail.isPending}
    <p class="text-gray-400">Loading repository...</p>
  {:else if repoDetail.error}
    <div class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300">
      {repoDetail.error.message}
    </div>
  {:else if repo}
    <!-- File browser -->
    {#if mainEnv && mainDesiredDeployment}
      <div class="mb-8">
        <h2 class="text-lg font-medium text-gray-300 mb-3">
          Files
          <span class="text-gray-500 text-sm font-normal ml-2">
            {mainEnv.name} &middot;
            <span class="font-mono"
              >{mainDesiredDeployment.commit.hash.substring(0, 8)}</span
            >
            &mdash; {mainDesiredDeployment.commit.message}
          </span>
        </h2>
        <RootTree
          {orgName}
          {repoName}
          commitHash={mainDesiredDeployment.commit.hash}
        />
      </div>
    {/if}

    <!-- Environments table -->
    <div>
      <h2 class="text-lg font-medium text-gray-300 mb-3">Environments</h2>
      {#if repo.environments.length === 0}
        <p class="text-sm text-gray-400">No environments found.</p>
      {:else}
        <table class="w-full text-sm text-left">
          <thead>
            <tr class="border-b border-gray-800 text-gray-400">
              <th class="pb-2 pr-4 font-medium">
                <button
                  class="hover:text-gray-200 transition-colors cursor-pointer"
                  onclick={() => toggleSort("name")}
                >
                  Name{sortIndicator("name")}
                </button>
              </th>
              <th class="pb-2 pr-4 font-medium">
                <button
                  class="hover:text-gray-200 transition-colors cursor-pointer"
                  onclick={() => toggleSort("deployment")}
                >
                  Current deployment{sortIndicator("deployment")}
                </button>
              </th>
              <th class="pb-2 font-medium">
                <button
                  class="hover:text-gray-200 transition-colors cursor-pointer"
                  onclick={() => toggleSort("resources")}
                >
                  Resources{sortIndicator("resources")}
                </button>
              </th>
            </tr>
          </thead>
          <tbody>
            {#each sortedEnvironments as env}
              {@const desired = desiredDeployment(env)}
              <tr class="border-b border-gray-800/50 hover:bg-gray-900/50">
                <td class="py-2 pr-4">
                  <a
                    href={envHref(orgName, repoName, env.name)}
                    class="text-blue-400 hover:text-blue-300"
                  >
                    {env.name}
                  </a>
                </td>
                <td class="py-2 pr-4 text-gray-400">
                  {#if desired}
                    <a
                      href={deploymentHref(orgName, repoName, env.name, desired.commit.hash)}
                      class="hover:text-gray-200 transition-colors"
                    >
                      <span class="font-mono text-blue-400 hover:text-blue-300"
                        >{desired.commit.hash.substring(0, 8)}</span
                      >
                      <span class="ml-2 truncate"
                        >{desired.commit.message.split("\n")[0]}</span
                      >
                    </a>
                  {:else}
                    <span class="text-gray-600">&mdash;</span>
                  {/if}
                </td>
                <td class="py-2">
                  <a
                    href={resourcesHref(orgName, repoName, env.name)}
                    class="text-blue-400 hover:text-blue-300"
                  >
                    {env.resources.length}
                  </a>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  {/if}
</div>
