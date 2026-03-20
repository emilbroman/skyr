<script lang="ts">
import { page } from "$app/stores";
import DeploymentStateBadge from "$lib/components/DeploymentState.svelte";
import RootTree from "$lib/components/RootTree.svelte";
import { DeploymentState, RepositoryDetailDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { envHref, orgHref } from "$lib/paths";

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
    <div class="flex gap-6">
      <!-- File browser (main column) -->
      <div class="flex-1 min-w-0">
        {#if mainEnv && mainDesiredDeployment}
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
        {/if}
      </div>

      <!-- Environments sidebar -->
      <aside class="w-72 shrink-0">
        <h2 class="text-lg font-medium text-gray-300 mb-3">Environments</h2>
        {#if repo.environments.length === 0}
          <p class="text-sm text-gray-400">No environments found.</p>
        {:else}
          <div class="grid gap-3">
            {#each repo.environments as env}
              {@const desired = env.deployments.find(
                (d) =>
                  d.state === DeploymentState.Desired ||
                  d.state === DeploymentState.Up,
              )}
              <a
                href={envHref(orgName, repoName, env.name)}
                class="block bg-gray-900 border border-gray-800 rounded-lg p-4 hover:border-gray-700 transition-colors"
              >
                <div class="flex items-center gap-2">
                  <h3 class="text-sm font-medium text-white">{env.name}</h3>
                  {#if desired}
                    <DeploymentStateBadge state={desired.state} />
                  {/if}
                </div>
                {#if desired}
                  <div
                    class="mt-1.5 text-xs text-gray-500 flex items-center gap-2"
                  >
                    <span class="font-mono"
                      >{desired.commit.hash.substring(0, 8)}</span
                    >
                    <span class="truncate">{desired.commit.message}</span>
                  </div>
                {/if}
                <div class="mt-1 text-xs text-gray-600">
                  {env.deployments.length} deployment{env.deployments.length !==
                  1
                    ? "s"
                    : ""}
                </div>
              </a>
            {/each}
          </div>
        {/if}
      </aside>
    </div>
  {/if}
</div>
