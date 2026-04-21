<script lang="ts">
import { page } from "$app/stores";
import DeploymentStateBadge from "$lib/components/DeploymentState.svelte";
import JsonTree from "$lib/components/JsonTree.svelte";
import LogStream from "$lib/components/LogStream.svelte";
import Spinner from "$lib/components/Spinner.svelte";
import { Copy, ExternalLink } from "lucide-svelte";
import { ResourceDetailDocument, ResourceLogsDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { commitTreeHref, decodeSegment, deploymentHref, resourceHref } from "$lib/paths";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let envName = $derived(decodeSegment($page.params.env ?? ""));
let resourceId = $derived(decodeSegment($page.params.resource ?? ""));

const resourceDetail = graphqlQuery(() => ({
    document: ResourceDetailDocument,
    variables: { org: orgName, repo: repoName, env: envName, resourceId },
    refetchInterval: 10_000,
}));

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

{#if resourceDetail.isPending}
  <Spinner />
{:else if resourceDetail.error}
  <div class="p-4 bg-red-50 border border-red-200 rounded text-red-600">
    {resourceDetail.error.message}
  </div>
{:else if resource}
  <!-- Header -->
  <div class="mb-6">
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
    </h2>
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
