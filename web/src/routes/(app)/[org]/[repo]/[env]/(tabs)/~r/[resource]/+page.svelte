<script lang="ts">
import { page } from "$app/stores";
import DeploymentStateBadge from "$lib/components/DeploymentState.svelte";
import HealthBadge from "$lib/components/HealthBadge.svelte";
import JsonTree from "$lib/components/JsonTree.svelte";
import LogStream from "$lib/components/LogStream.svelte";
import Spinner from "$lib/components/Spinner.svelte";
import { Copy, ExternalLink, Loader2, Trash2 } from "lucide-svelte";
import {
    DeleteResourceDocument,
    ResourceDetailDocument,
    ResourceLogsDocument,
    ResourceMarker,
} from "$lib/graphql/generated";
import { graphqlMutation, graphqlQuery } from "$lib/graphql/query";
import {
    commitTreeHref,
    decodeSegment,
    deploymentHref,
    orgIncidentHref,
    resourceHref,
} from "$lib/paths";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let envName = $derived(decodeSegment($page.params.env ?? ""));
let resourceId = $derived(decodeSegment($page.params.resource ?? ""));

const resourceDetail = graphqlQuery(() => ({
    document: ResourceDetailDocument,
    variables: { org: orgName, repo: repoName, env: envName, resourceId },
    refetchInterval: 10_000,
}));

const DESTROYING_TIMEOUT_MS = 10 * 60 * 1000;
let destroyingSince = $state<number | null>(null);
let destroyingTimeoutId: ReturnType<typeof setTimeout> | null = null;
let deleteConfirmOpen = $state(false);
let deleteConfirmInput = $state("");
let deleteError = $state<string | null>(null);

function clearDestroyingTimeout() {
    if (destroyingTimeoutId !== null) {
        clearTimeout(destroyingTimeoutId);
        destroyingTimeoutId = null;
    }
}

const deleteResource = graphqlMutation(DeleteResourceDocument, {
    onSuccess: () => {
        deleteConfirmOpen = false;
        deleteConfirmInput = "";
        deleteError = null;
        destroyingSince = Date.now();
        clearDestroyingTimeout();
        destroyingTimeoutId = setTimeout(() => {
            destroyingSince = null;
            destroyingTimeoutId = null;
        }, DESTROYING_TIMEOUT_MS);
        resourceDetail.refetch();
    },
    onError: (e) => {
        deleteError = e.message;
    },
});

let isDestroying = $derived(destroyingSince !== null);

function handleDeleteClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (!target.closest(".delete-dropdown")) {
        deleteConfirmOpen = false;
        deleteConfirmInput = "";
        deleteError = null;
    }
}

let envQid = $derived(resourceDetail.data?.organization.repository.environment.qid ?? "");
let resource = $derived(resourceDetail.data?.organization.repository.environment.resource ?? null);
let resourceQid = $derived(envQid && resourceId ? `${envQid}::${resourceId}` : "");

let typeParts = $derived(resource?.type.split(".") ?? []);

function moduleIdToLocalPath(moduleId: string): string {
    const segments = moduleId.split("/");
    return segments.length > 2 ? segments.slice(2).join("/") : moduleId;
}

function parseSpanStartLine(span: string): number {
    const startPart = span.split(",")[0];
    const line = parseInt(startPart.split(":")[0], 10);
    return Number.isNaN(line) ? 1 : line;
}

/** Extract a string from a serde-tagged Value ({"Str": "..."} or bare string). */
function extractStr(val: unknown): string | null {
    if (typeof val === "string") return val;
    if (val && typeof val === "object" && "Str" in val) return (val as { Str: string }).Str;
    return null;
}

/** Extract an integer from a serde-tagged Value ({"Int": N} or bare number). */
function extractInt(val: unknown): number | null {
    if (typeof val === "number") return val;
    if (val && typeof val === "object" && "Int" in val) return (val as { Int: number }).Int;
    return null;
}

/** Get a field from a bare record ({fields: {...}}) or a tagged Record. */
function getField(record: unknown, key: string): unknown | null {
    if (!record || typeof record !== "object") return null;
    const obj = record as Record<string, unknown>;
    if (obj.fields && typeof obj.fields === "object" && !Array.isArray(obj.fields)) {
        return (obj.fields as Record<string, unknown>)[key] ?? null;
    }
    if (obj.Record && typeof obj.Record === "object") {
        const inner = obj.Record as Record<string, unknown>;
        if (inner.fields && typeof inner.fields === "object") {
            return (inner.fields as Record<string, unknown>)[key] ?? null;
        }
    }
    return null;
}

const PORT_TYPES = ["Std/Container.Pod.Port", "Std/Container.Host.Port"];
const DNS_A_RECORD_TYPE = "Std/DNS.ARecord";

let isPortResource = $derived(resource ? PORT_TYPES.includes(resource.type) : false);
let isDnsARecord = $derived(resource?.type === DNS_A_RECORD_TYPE);

let portForwardPort = $derived.by(() => {
    if (!isPortResource || !resource?.inputs) return null;
    const port = extractInt(getField(resource.inputs, "port"));
    if (port == null) return null;
    return port > 1024 ? port : port + 5000;
});

let aRecordFqdn = $derived.by(() => {
    if (!isDnsARecord || !resource?.outputs) return null;
    return extractStr(getField(resource.outputs, "fqdn"));
});
</script>

<svelte:head>
    <title>{resourceId} · {orgName}/{repoName} ({envName}) – Skyr</title>
</svelte:head>

<svelte:window onclick={handleDeleteClickOutside} />

{#if resourceDetail.isPending}
  <Spinner />
{:else if resourceDetail.error}
  <div class="p-4 bg-red-50 border border-red-200 rounded text-red-600">
    {resourceDetail.error.message}
  </div>
{:else if resource}
  <!-- Header -->
  <div class="mb-6 flex items-start gap-4">
    <div class="flex-1">
      <div class="mb-1">
        {#if typeParts.length > 1}
          <span class="text-orange-500/70"
            >{typeParts.slice(0, -1).join(".")}.</span
          >
        {/if}
        <span class="text-orange-500">{typeParts[typeParts.length - 1]}</span>
      </div>
      <h2 class="font-bold text-gray-900 flex items-center gap-2">
        {resource.name}
        {#each resource.markers as marker}
          <span
            class="px-1.5 py-px rounded border {marker === 'VOLATILE'
              ? 'border-yellow-300 text-yellow-700'
              : 'border-blue-300 text-blue-700'}"
          >
            {marker}
          </span>
        {/each}
        {#if isDestroying}
          <span
            class="px-1.5 py-px rounded border border-red-300 text-red-700 inline-flex items-center gap-1"
            title="Destroy message enqueued; waiting for plugin confirmation"
          >
            <Loader2 class="w-3 h-3 animate-spin" />
            Destroying…
          </span>
        {/if}
      </h2>
    </div>

    <div class="delete-dropdown relative inline-block">
      <button
        type="button"
        class="inline-flex items-center gap-1.5 bg-white border border-gray-200 rounded-lg px-3 py-1.5 text-gray-600 font-medium cursor-pointer hover:border-red-400 hover:text-red-600 transition-colors focus:outline-none focus:border-red-500 disabled:opacity-50 disabled:cursor-not-allowed"
        disabled={isDestroying}
        onclick={() => {
            deleteConfirmOpen = !deleteConfirmOpen;
            deleteConfirmInput = "";
            deleteError = null;
        }}
      >
        <Trash2 class="w-4 h-4" />
        Delete
      </button>

      {#if deleteConfirmOpen}
        <div
          class="absolute right-0 mt-1 z-10 bg-white border border-gray-200 rounded-lg shadow-lg p-4 w-96"
        >
          <p class="text-sm font-medium text-gray-700 mb-2">Delete resource?</p>
          <p class="text-sm text-gray-500 mb-3">
            This will ask the resource's plugin to destroy
            <code class="bg-gray-100 px-1 py-0.5 rounded text-xs">{resource.type}:{resource.name}</code>.
            If the owning deployment is still
            <span class="font-medium">Desired</span>, the next evaluation tick
            may recreate the resource.
          </p>
          {#if resource.markers.includes(ResourceMarker.Sticky)}
            <p class="text-sm text-blue-700 mb-3">
              This resource is marked <code class="bg-blue-50 px-1 py-0.5 rounded text-xs">STICKY</code>
              — it will not be recreated automatically.
            </p>
          {/if}
          <label for="delete-confirm" class="block text-sm text-gray-600 mb-1">
            Type <code class="bg-gray-100 px-1 py-0.5 rounded text-xs select-all">{resource.name}</code> to confirm
          </label>
          <input
            id="delete-confirm"
            type="text"
            class="w-full text-sm border border-gray-200 rounded-lg px-2.5 py-1.5 mb-3 focus:outline-none focus:border-red-500"
            bind:value={deleteConfirmInput}
            placeholder={resource.name}
          />
          {#if deleteError}
            <div class="mb-3 p-2 bg-red-50 border border-red-200 rounded text-sm text-red-600">
              {deleteError}
            </div>
          {/if}
          <div class="flex gap-2 justify-end">
            <button
              type="button"
              class="px-3 py-1.5 text-sm rounded-lg border border-gray-200 text-gray-600 hover:border-gray-400 transition-colors cursor-pointer"
              onclick={() => {
                  deleteConfirmOpen = false;
                  deleteConfirmInput = "";
                  deleteError = null;
              }}
            >
              Cancel
            </button>
            <button
              type="button"
              class="px-3 py-1.5 text-sm rounded-lg bg-red-600 text-white hover:bg-red-700 transition-colors cursor-pointer disabled:opacity-50"
              disabled={deleteResource.isPending || deleteConfirmInput !== resource.name}
              onclick={() => deleteResource.mutate({
                  org: orgName,
                  repo: repoName,
                  env: envName,
                  resource: resourceId,
              })}
            >
              {deleteResource.isPending ? "Requesting delete..." : "Confirm delete"}
            </button>
          </div>
        </div>
      {/if}
    </div>
  </div>

  <!-- Metadata -->
  <div class="bg-white border border-gray-200 rounded-lg p-4 mb-6">
    <dl class="grid grid-cols-2 gap-x-6 gap-y-3">
      {#if resourceQid}
        <div>
          <dt class="text-gray-400">QID</dt>
          <dd class="font-mono text-xs text-gray-600 flex items-center gap-1.5">
            {resourceQid}
            <button
              type="button"
              class="text-gray-400 hover:text-gray-600 transition-colors"
              title="Copy QID"
              onclick={() => navigator.clipboard.writeText(resourceQid)}
            >
              <Copy class="w-3.5 h-3.5" />
            </button>
          </dd>
        </div>
      {/if}
      {#if resource.owner}
        <div>
          <dt class="text-gray-400">Owner</dt>
          <dd>
            <a
              href={deploymentHref(
                orgName,
                repoName,
                envName,
                `${resource.owner.commit.hash}.${resource.owner.nonce}`,
              )}
              class="text-orange-600 hover:text-orange-500 font-mono text-xs transition-colors"
            >
              {resource.owner.commit.hash.substring(0, 8)}
            </a>
            <span class="text-gray-500 ml-2"
              >{resource.owner.commit.message.split("\n")[0]}</span
            >
          </dd>
        </div>
        <div>
          <dt class="text-gray-400">Deployment State</dt>
          <dd><DeploymentStateBadge state={resource.owner.state} bootstrapped={resource.owner.bootstrapped} failures={resource.owner.failures} /></dd>
        </div>
      {/if}
      <div>
        <dt class="text-gray-400">Health</dt>
        <dd>
          <HealthBadge
            health={resource.status.health}
            openIncidentCount={resource.status.openIncidentCount}
            worstOpenCategory={resource.status.worstOpenCategory}
          />
        </dd>
      </div>
      <div>
        <dt class="text-gray-400">Last report</dt>
        <dd class="text-gray-700">
          {new Date(resource.status.lastReportAt).getTime() === 0
            ? "Never"
            : new Date(resource.status.lastReportAt).toLocaleString()}
        </dd>
      </div>
      {#if resource.sourceTrace.length > 0}
        {@const frame = resource.sourceTrace[0]}
        {@const filePath = moduleIdToLocalPath(frame.moduleId) + ".scl"}
        {@const line = parseSpanStartLine(frame.span)}
        <div>
          <dt class="text-gray-400">Source</dt>
          <dd>
            <a
              href={commitTreeHref(
                orgName,
                repoName,
                resource.owner?.commit.hash ?? "",
                filePath,
              ) + `#line-${line}`}
              class="text-orange-600 hover:text-orange-500 font-mono text-xs transition-colors"
            >
              {filePath}:{line}
            </a>
          </dd>
        </div>
      {/if}
    </dl>
    {#if resource.incidents.length > 0}
      <div class="mt-4">
        <h3 class="text-sm font-medium text-gray-700 mb-2">Incidents</h3>
        <ul class="space-y-1 text-sm">
          {#each resource.incidents as incident}
            <li>
              <a
                href={orgIncidentHref(orgName, incident.id)}
                class="text-orange-600 hover:text-orange-500"
              >
                {incident.category} · opened {new Date(incident.openedAt).toLocaleString()}
                {incident.closedAt
                  ? ` · closed ${new Date(incident.closedAt).toLocaleString()}`
                  : " · OPEN"}
              </a>
            </li>
          {/each}
        </ul>
      </div>
    {/if}
  </div>

  <!-- Resource Widgets -->
  {#if isPortResource && resourceQid && portForwardPort != null}
    <section class="mb-6">
      <h3 class="font-medium text-gray-600 mb-3">Port Forward</h3>
      <div class="bg-white border border-gray-200 rounded-lg p-4">
        <div class="flex items-center gap-2 font-mono text-xs text-gray-600 bg-gray-50 rounded px-3 py-2">
          <code class="flex-1 select-all">skyr port-forward {resourceQid} {portForwardPort}</code>
          <button
            type="button"
            class="text-gray-400 hover:text-gray-600 transition-colors shrink-0"
            title="Copy command"
            onclick={() => navigator.clipboard.writeText(`skyr port-forward ${resourceQid} ${portForwardPort}`)}
          >
            <Copy class="w-3.5 h-3.5" />
          </button>
        </div>
      </div>
    </section>
  {/if}
  {#if isDnsARecord && aRecordFqdn}
    <section class="mb-6">
      <h3 class="font-medium text-gray-600 mb-3">DNS</h3>
      <div class="bg-white border border-gray-200 rounded-lg p-4">
        <div class="flex items-center gap-2">
          <span class="font-mono text-sm text-gray-600">{aRecordFqdn}</span>
          <a
            href="http://{aRecordFqdn}"
            target="_blank"
            rel="noopener noreferrer"
            class="text-orange-600 hover:text-orange-500 transition-colors"
            title="Open in browser"
          >
            <ExternalLink class="w-4 h-4" />
          </a>
        </div>
      </div>
    </section>
  {/if}

  <!-- Dependencies -->
  {#if resource.dependencies.length > 0}
    <section class="mb-6">
      <h3 class="font-medium text-gray-600 mb-3">Dependencies</h3>
      <div class="flex flex-wrap gap-1.5">
        {#each resource.dependencies as dep}
          <a
            href={resourceHref(
              orgName,
              repoName,
              envName,
              `${dep.type}:${dep.name}`,
            )}
            class="px-2 py-1 bg-gray-100 border border-gray-300 rounded text-gray-600 hover:text-gray-900 hover:border-gray-400 transition-colors"
          >
            {dep.type}::{dep.name}
          </a>
        {/each}
      </div>
    </section>
  {/if}

  <!-- Inputs -->
  {#if resource.inputs != null}
    <section class="mb-6">
      <h3 class="font-medium text-gray-600 mb-3">Inputs</h3>
      <div
        class="bg-white border border-gray-200 rounded-lg p-4 text-gray-600 font-mono text-xs overflow-x-auto"
      >
        <JsonTree value={resource.inputs} />
      </div>
    </section>
  {/if}

  <!-- Outputs -->
  {#if resource.outputs != null}
    <section class="mb-6">
      <h3 class="font-medium text-gray-600 mb-3">Outputs</h3>
      <div
        class="bg-white border border-gray-200 rounded-lg p-4 text-gray-600 font-mono text-xs overflow-x-auto"
      >
        <JsonTree value={resource.outputs} />
      </div>
    </section>
  {/if}

  <!-- Logs -->
  {#if resourceQid}
    <section>
      <h3 class="font-medium text-gray-600 mb-3">Logs</h3>
      <div
        class="h-96 bg-white border border-gray-200 rounded-lg overflow-hidden"
      >
        <LogStream
          document={ResourceLogsDocument}
          variables={{ resourceQid, initialAmount: 100 }}
          logField="resourceLogs"
        />
      </div>
    </section>
  {/if}
{:else}
  <p class="text-gray-500">Resource not found.</p>
{/if}
