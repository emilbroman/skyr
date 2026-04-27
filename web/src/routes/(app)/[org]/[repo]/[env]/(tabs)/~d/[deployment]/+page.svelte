<script lang="ts">
import { page } from "$app/stores";
import DeploymentStateBadge from "$lib/components/DeploymentState.svelte";
import HealthBadge from "$lib/components/HealthBadge.svelte";
import { envIncidentHref } from "$lib/paths";
import LogStream from "$lib/components/LogStream.svelte";
import Spinner from "$lib/components/Spinner.svelte";
import ResourceList from "$lib/components/ResourceList.svelte";
import CommitMessage from "$lib/components/CommitMessage.svelte";
import { ArrowUpRight } from "lucide-svelte";
import {
    CreateDeploymentDocument,
    DeploymentDetailDocument,
    DeploymentLogsDocument,
    DeploymentState,
    ResourceMarker,
} from "$lib/graphql/generated";
import { graphqlMutation, graphqlQuery } from "$lib/graphql/query";
import { commitTreeHref, decodeSegment } from "$lib/paths";
import { formatLogTimestamp } from "$lib/timestamps";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let envName = $derived(decodeSegment($page.params.env ?? ""));
let deploymentId = $derived($page.params.deployment ?? "");

const deploymentDetail = graphqlQuery(() => ({
    document: DeploymentDetailDocument,
    variables: {
        org: orgName,
        repo: repoName,
        env: envName,
        id: deploymentId,
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
    if (!deployment) return;
    createDeploymentError = null;
    createDeployment.mutate({
        org: orgName,
        repo: repoName,
        env: envName,
        commitHash: deployment.commit.hash,
    });
}
</script>

<svelte:head>
    <title>{deployment?.shortId ?? deploymentId} · {orgName}/{repoName} ({envName}) – Skyr</title>
</svelte:head>

<div>
  {#if deploymentDetail.isPending}
    <Spinner />
  {:else if deploymentDetail.error}
    <div class="p-4 bg-red-50 border border-red-200 rounded text-red-600">
      {deploymentDetail.error.message}
    </div>
  {:else if deployment}
    <!-- Commit -->
    <div class="bg-white border border-gray-200 rounded-lg p-4 mb-6">
      <div class="text-xs mb-2">
        <a
          href={commitTreeHref(orgName, repoName, deployment.commit.hash)}
          class="text-blue-600 hover:text-blue-500 transition-colors inline-flex items-center gap-0.5"
        >
          <span class="font-mono">{deployment.commit.hash.slice(0, 8)}</span>
          <ArrowUpRight class="w-3 h-3" />
        </a>
      </div>
      <CommitMessage message={deployment.commit.message} />
    </div>

    <!-- Metadata -->
    <div class="bg-white border border-gray-200 rounded-lg p-4 mb-6">
      <dl class="grid grid-cols-2 gap-x-6 gap-y-3">
        <div>
          <dt class="text-gray-400">Ref</dt>
          <dd class="text-gray-700">{envName}</dd>
        </div>
        <div>
          <dt class="text-gray-400">Created</dt>
          <dd class="text-gray-700">
            {new Date(deployment.createdAt).toLocaleString()}
          </dd>
        </div>
        <div>
          <dt class="text-gray-400">State</dt>
          <dd><DeploymentStateBadge state={deployment.state} bootstrapped={deployment.bootstrapped} volatile={hasVolatile} /></dd>
        </div>
        <div>
          <dt class="text-gray-400">Health</dt>
          <dd>
            <HealthBadge
              health={deployment.status.health}
              openIncidentCount={deployment.status.openIncidentCount}
            />
          </dd>
        </div>
        <div>
          <dt class="text-gray-400">Last report</dt>
          <dd class="text-gray-700">
            {new Date(deployment.status.lastReportAt).getTime() === 0
              ? "Never"
              : new Date(deployment.status.lastReportAt).toLocaleString()}
          </dd>
        </div>
        <div>
          <dt class="text-gray-400">Open incidents</dt>
          <dd class="text-gray-700">{deployment.status.openIncidentCount}</dd>
        </div>
      </dl>
      {#if deployment.openIncidents.length > 0}
        <div class="mt-4">
          <h3 class="text-sm font-medium text-gray-700 mb-2">Open incidents</h3>
          <ul class="space-y-1 text-sm">
            {#each deployment.openIncidents as incident}
              <li>
                <a
                  href={envIncidentHref(orgName, repoName, envName, incident.id)}
                  class="text-blue-600 hover:text-blue-500"
                >
                  Opened {new Date(incident.openedAt).toLocaleString()} · OPEN
                </a>
              </li>
            {/each}
          </ul>
        </div>
      {/if}
      {#if canRedeploy}
        <div class="mt-3 flex items-center gap-4">
          <button
            type="button"
            onclick={onRedeploy}
            disabled={createDeployment.isPending}
            class="px-3 py-1 text-xs font-medium text-white bg-gray-900 hover:bg-gray-800 rounded transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {createDeployment.isPending ? "Deploying..." : "Deploy"}
          </button>
        </div>
      {/if}
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
            variables={{ deploymentId: deployment.qid, initialAmount: 100 }}
            logField="deploymentLogs"
          />
        </div>
      {:else if deployment.lastLogs.length > 0}
        <div
          class="bg-gray-900 border border-gray-800 rounded-lg p-3 font-mono text-xs space-y-0.5 max-h-60 overflow-y-auto"
        >
          {#each deployment.lastLogs as log}
            <div class="flex flex-col sm:flex-row sm:gap-2 leading-5">
              <span class="text-gray-500 shrink-0"
                >{formatLogTimestamp(log.timestamp)}</span
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
      <ResourceList
        resources={deployment.resources}
        org={orgName}
        repo={repoName}
        env={envName}
        emptyMessage="No resources in this deployment."
      >
        {#snippet header()}
          <h2 class="font-medium text-gray-600">
            Resources
            <span class="text-gray-400 font-normal ml-1"
              >({deployment.resources.length})</span
            >
          </h2>
        {/snippet}
      </ResourceList>
    </section>
  {/if}
</div>
