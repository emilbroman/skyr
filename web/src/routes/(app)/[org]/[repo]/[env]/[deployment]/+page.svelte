<script lang="ts">
import { page } from "$app/stores";
import DeploymentStateBadge from "$lib/components/DeploymentState.svelte";
import LogStream from "$lib/components/LogStream.svelte";
import ResourceDag from "$lib/components/ResourceDag.svelte";
import {
    DeploymentDetailDocument,
    DeploymentLogsDocument,
    DeploymentState,
} from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { commitTreeHref, decodeSegment, envHref, resourcesHref } from "$lib/paths";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let envName = $derived(decodeSegment($page.params.env ?? ""));
let commitHash = $derived($page.params.deployment ?? "");

const deploymentDetail = graphqlQuery(() => ({
    document: DeploymentDetailDocument,
    variables: {
        org: orgName,
        repo: repoName,
        env: envName,
        commit: commitHash,
    },
    refetchInterval: 10_000,
}));

let deployment = $derived(
    deploymentDetail.data?.organization.repository.environment.deployment ?? null,
);

const liveStates: DeploymentState[] = [
    DeploymentState.Desired,
    DeploymentState.Lingering,
    DeploymentState.Undesired,
];
let isLive = $derived(deployment != null && liveStates.includes(deployment.state));
</script>

<div>
  <nav class="text-sm text-gray-500 mb-4">
    <a href={envHref(orgName, repoName, envName)} class="hover:text-gray-300"
      >{envName}</a
    >
    <span class="mx-2">/</span>
    <span class="text-gray-300 font-mono text-xs"
      >{commitHash.substring(0, 8)}</span
    >
  </nav>

  {#if deploymentDetail.isPending}
    <p class="text-gray-400">Loading deployment...</p>
  {:else if deploymentDetail.error}
    <div class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300">
      {deploymentDetail.error.message}
    </div>
  {:else if deployment}
    <!-- Header -->
    <div class="flex items-center gap-4 mb-6">
      <DeploymentStateBadge state={deployment.state} />
      <h1 class="text-xl font-bold text-white font-mono">
        {deployment.commit.hash.substring(0, 8)}
      </h1>
    </div>

    <!-- Metadata -->
    <div class="bg-gray-900 border border-gray-800 rounded-lg p-4 mb-6">
      <dl class="grid grid-cols-2 gap-x-6 gap-y-3 text-sm">
        <div>
          <dt class="text-gray-500">Ref</dt>
          <dd class="text-gray-200">{deployment.ref}</dd>
        </div>
        <div>
          <dt class="text-gray-500">Commit</dt>
          <dd class="text-gray-200 font-mono" title={deployment.commit.message}>
            {deployment.commit.hash.substring(0, 8)} &mdash; {deployment.commit
              .message}
          </dd>
        </div>
        <div>
          <dt class="text-gray-500">Created</dt>
          <dd class="text-gray-200">
            {new Date(deployment.createdAt).toLocaleString()}
          </dd>
        </div>
        <div>
          <dt class="text-gray-500">State</dt>
          <dd class="text-gray-200">{deployment.state}</dd>
        </div>
      </dl>
      <a
        href={commitTreeHref(orgName, repoName, deployment.commit.hash)}
        class="inline-block mt-3 text-sm text-indigo-400 hover:text-indigo-300 transition-colors"
      >
        View files &rarr;
      </a>
    </div>

    <!-- Artifacts -->
    {#if deployment.artifacts.length > 0}
      <section class="mb-6">
        <h2 class="text-lg font-medium text-gray-300 mb-3">Artifacts</h2>
        <div class="space-y-2">
          {#each deployment.artifacts as artifact}
            <div
              class="bg-gray-900 border border-gray-800 rounded-lg px-4 py-3 flex items-center justify-between"
            >
              <div>
                <span class="text-gray-200 text-sm">{artifact.name}</span>
                <span class="text-gray-500 text-xs ml-2"
                  >({artifact.mediaType})</span
                >
              </div>
              <a
                href={artifact.url}
                target="_blank"
                rel="noopener noreferrer"
                class="text-indigo-400 hover:text-indigo-300 text-sm"
              >
                Download
              </a>
            </div>
          {/each}
        </div>
      </section>
    {/if}

    <!-- Resources -->
    <section class="mb-6">
      <a
        href={resourcesHref(orgName, repoName, envName)}
        class="text-lg font-medium text-gray-300 mb-3 block hover:text-white transition-colors"
      >
        Resources
        <span class="text-gray-500 text-sm font-normal ml-1"
          >({deployment.resources.length})</span
        >
      </a>
      <ResourceDag
        resources={deployment.resources}
        org={orgName}
        repo={repoName}
        env={envName}
      />
    </section>

    <!-- Logs -->
    <section>
      <h2 class="text-lg font-medium text-gray-300 mb-3">Logs</h2>
      {#if isLive}
        <div
          class="h-96 bg-gray-900 border border-gray-800 rounded-lg overflow-hidden"
        >
          <LogStream
            document={DeploymentLogsDocument}
            variables={{ deploymentId: deployment.id, initialAmount: 100 }}
            logField="deploymentLogs"
          />
        </div>
      {:else if deployment.lastLogs.length > 0}
        <div
          class="bg-gray-900 border border-gray-800 rounded-lg p-3 font-mono text-xs space-y-0.5 max-h-60 overflow-y-auto"
        >
          {#each deployment.lastLogs as log}
            <div class="flex gap-2 leading-5">
              <span class="text-gray-500 shrink-0"
                >{new Date(log.timestamp).toLocaleTimeString()}</span
              >
              <span
                class={log.severity === "ERROR"
                  ? "text-red-400"
                  : log.severity === "WARNING"
                    ? "text-yellow-400"
                    : "text-gray-300"}>{log.message}</span
              >
            </div>
          {/each}
        </div>
      {:else}
        <p class="text-gray-500">No logs available.</p>
      {/if}
    </section>
  {/if}
</div>
