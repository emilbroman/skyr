<script lang="ts">
import { page } from "$app/stores";
import DeploymentStateBadge from "$lib/components/DeploymentState.svelte";
import LogStream from "$lib/components/LogStream.svelte";
import Spinner from "$lib/components/Spinner.svelte";
import ResourceDag from "$lib/components/ResourceDag.svelte";
import {
    CreateDeploymentDocument,
    DeploymentDetailDocument,
    DeploymentLogsDocument,
    DeploymentState,
    ResourceMarker,
} from "$lib/graphql/generated";
import { graphqlMutation, graphqlQuery } from "$lib/graphql/query";
import { commitTreeHref, decodeSegment } from "$lib/paths";

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
let hasVolatile = $derived(
    deployment?.resources.some((r) => r.markers.includes(ResourceMarker.Volatile)) ?? false,
);

let createDeploymentError = $state<string | null>(null);

const createDeployment = graphqlMutation(CreateDeploymentDocument, {
    onSuccess: () => {
        createDeploymentError = null;
        deploymentDetail.refetch();
    },
    onError: (e) => {
        createDeploymentError = e.message;
    },
});

let canRedeploy = $derived(deployment != null && deployment.state !== DeploymentState.Desired);

function onRedeploy() {
    createDeploymentError = null;
    createDeployment.mutate({
        org: orgName,
        repo: repoName,
        env: envName,
        commitHash,
    });
}
</script>

<svelte:head>
    <title>{commitHash.substring(0, 8)} · {orgName}/{repoName} ({envName}) – Skyr</title>
</svelte:head>

<div>
  {#if deploymentDetail.isPending}
    <Spinner />
  {:else if deploymentDetail.error}
    <div class="p-4 bg-red-50 border border-red-200 rounded text-red-600">
      {deploymentDetail.error.message}
    </div>
  {:else if deployment}
    <!-- Metadata -->
    <div class="bg-white border border-gray-200 rounded-lg p-4 mb-6">
      <dl class="grid grid-cols-2 gap-x-6 gap-y-3">
        <div>
          <dt class="text-gray-400">Ref</dt>
          <dd class="text-gray-700">{deployment.ref}</dd>
        </div>
        <div>
          <dt class="text-gray-400">Commit</dt>
          <dd class="text-gray-700 font-mono text-xs" title={deployment.commit.message}>
            {deployment.commit.hash.substring(0, 8)} &mdash; {deployment.commit
              .message}
          </dd>
        </div>
        <div>
          <dt class="text-gray-400">Created</dt>
          <dd class="text-gray-700">
            {new Date(deployment.createdAt).toLocaleString()}
          </dd>
        </div>
        <div>
          <dt class="text-gray-400">State</dt>
          <dd><DeploymentStateBadge state={deployment.state} bootstrapped={deployment.bootstrapped} failures={deployment.failures} volatile={hasVolatile} /></dd>
        </div>
      </dl>
      <div class="mt-3 flex items-center gap-4">
        <a
          href={commitTreeHref(orgName, repoName, deployment.commit.hash)}
          class="text-orange-600 hover:text-orange-500 transition-colors"
        >
          View files &rarr;
        </a>
        {#if canRedeploy}
          <button
            type="button"
            onclick={onRedeploy}
            disabled={createDeployment.isPending}
            class="px-3 py-1 bg-orange-600 hover:bg-orange-500 text-gray-900 rounded font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {createDeployment.isPending ? "Deploying..." : "Deploy"}
          </button>
        {/if}
      </div>
      {#if createDeploymentError}
        <div
          class="mt-3 p-3 bg-red-50 border border-red-200 rounded text-red-600"
        >
          {createDeploymentError}
        </div>
      {/if}
    </div>

    <!-- Logs -->
    <section class="mb-6">
      <h2 class="font-medium text-gray-600 mb-3">Logs</h2>
      {#if isLive}
        <div
          class="h-96 bg-white border border-gray-200 rounded-lg overflow-hidden"
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
        <p class="text-gray-400">No logs available.</p>
      {/if}
    </section>

    <!-- Resources -->
    <section>
      <h2 class="font-medium text-gray-600 mb-3">
        Resources
        <span class="text-gray-400 font-normal ml-1"
          >({deployment.resources.length})</span
        >
      </h2>
      <ResourceDag
        resources={deployment.resources}
        org={orgName}
        repo={repoName}
        env={envName}
      />
    </section>
  {/if}
</div>
