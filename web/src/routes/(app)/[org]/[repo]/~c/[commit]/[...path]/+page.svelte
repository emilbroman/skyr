<script lang="ts">
import { goto, replaceState } from "$app/navigation";
import { page } from "$app/stores";
import { untrack } from "svelte";
import DirectoryView from "$lib/components/DirectoryView.svelte";
import Spinner from "$lib/components/Spinner.svelte";
import FileView from "$lib/components/FileView.svelte";
import CommitMessage from "$lib/components/CommitMessage.svelte";
import { ChevronDown, Rocket } from "lucide-svelte";
import { execute, query } from "$lib/graphql/client";
import {
    CommitPageEnvironmentsDocument,
    CommitRootTreeDocument,
    CommitTreeEntryDocument,
    CreateDeploymentDocument,
} from "$lib/graphql/generated";
import { commitTreeHref, deploymentHref } from "$lib/paths";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let commitHash = $derived($page.params.commit ?? "");
let pathParam = $derived($page.params.path ?? "");
let pathSegments = $derived(pathParam ? pathParam.split("/").filter(Boolean) : []);
let isRoot = $derived(pathSegments.length === 0);

type TreeEntry =
    | { __typename: "Tree"; hash: string; name?: string | null }
    | { __typename: "Blob"; hash: string; name?: string | null; size: number };

type ViewState =
    | { kind: "loading" }
    | { kind: "error"; message: string }
    | {
          kind: "directory";
          entries: TreeEntry[];
          commitMessage: string;
          parents: { hash: string }[];
      }
    | {
          kind: "file";
          content: string | null;
          size: number;
          commitMessage: string;
          parents: { hash: string }[];
      };

let view = $state<ViewState>({ kind: "loading" });

async function loadRoot() {
    view = { kind: "loading" };
    try {
        const data = await query(CommitRootTreeDocument, {
            org: orgName,
            repo: repoName,
            commit: commitHash,
        });
        const commit = data.organization.repository.commit;
        view = {
            kind: "directory",
            entries: commit.tree.entries,
            commitMessage: commit.message,
            parents: commit.parents,
        };
    } catch (e) {
        view = {
            kind: "error",
            message: e instanceof Error ? e.message : "Failed to load tree",
        };
    }
}

async function loadPath(path: string) {
    view = { kind: "loading" };
    try {
        const data = await query(CommitTreeEntryDocument, {
            org: orgName,
            repo: repoName,
            commit: commitHash,
            path,
        });
        const commit = data.organization.repository.commit;
        const entry = commit.treeEntry;
        if (!entry) {
            view = { kind: "error", message: `Path "${path}" not found` };
            return;
        }
        if (entry.__typename === "Tree") {
            view = {
                kind: "directory",
                entries: entry.entries,
                commitMessage: commit.message,
                parents: commit.parents,
            };
        } else {
            view = {
                kind: "file",
                content: entry.content ?? null,
                size: entry.size,
                commitMessage: commit.message,
                parents: commit.parents,
            };
        }
    } catch (e) {
        view = {
            kind: "error",
            message: e instanceof Error ? e.message : "Failed to load path",
        };
    }
}

let environments = $state<{ name: string }[]>([]);
let environmentsLoaded = $state(false);

async function loadEnvironments() {
    try {
        const data = await execute(CommitPageEnvironmentsDocument, {
            org: orgName,
            repo: repoName,
        });
        environments = data.organization.repository.environments;
    } catch {
        environments = [];
    } finally {
        environmentsLoaded = true;
    }
}

$effect(() => {
    const org = orgName;
    const repo = repoName;
    untrack(() => {
        if (org && repo) loadEnvironments();
    });
});

let deployMenuOpen = $state(false);
let deployConfirmEnv = $state<string | null>(null);
let deployError = $state<string | null>(null);
let deployPending = $state(false);

async function onDeploy(envName: string) {
    deployError = null;
    deployPending = true;
    try {
        const data = await execute(CreateDeploymentDocument, {
            org: orgName,
            repo: repoName,
            env: envName,
            commitHash,
        });
        const deployment = data.createDeployment;
        goto(deploymentHref(orgName, repoName, envName, deployment.id));
    } catch (e) {
        deployError = e instanceof Error ? e.message : "Failed to create deployment";
        deployPending = false;
    }
}

// Use mousedown (not click) so the check runs before any inside-click
// handler mutates state and re-renders. Otherwise, clicking a menu item
// can detach its button from the DOM before this handler sees it, making
// `target.closest(".deploy-dropdown")` return null and incorrectly close
// the dropdown.
function handleDeployMousedownOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (!target.closest(".deploy-dropdown")) {
        deployMenuOpen = false;
        deployConfirmEnv = null;
        deployError = null;
    }
}

let parents = $derived(view.kind === "directory" || view.kind === "file" ? view.parents : []);

// Load data when org/repo/commit/path changes.
// Uses untrack for the load calls so that setting `view` doesn't
// re-subscribe this effect to anything new.
$effect(() => {
    // Subscribe to the reactives that should trigger a reload:
    orgName;
    repoName;
    commitHash;
    const root = isRoot;
    const segments = pathSegments;
    untrack(() => {
        if (root) {
            loadRoot();
        } else {
            loadPath(segments.join("/"));
        }
    });
});

// Trailing slash normalization: directories get trailing slash, files lose it.
// Only depends on `view.kind`; reads the current URL inside untrack to avoid
// a feedback loop with replaceState updating $page.
$effect(() => {
    const kind = view.kind;
    untrack(() => {
        const currentUrl = $page.url.pathname;
        if (kind === "directory" && !currentUrl.endsWith("/")) {
            replaceState(`${currentUrl}/`, {});
        } else if (kind === "file" && currentUrl.endsWith("/")) {
            replaceState(currentUrl.replace(/\/+$/, ""), {});
        }
    });
});

let commitMessage = $derived(
    view.kind === "directory" || view.kind === "file" ? view.commitMessage : "",
);

// Parse #line-N hash for line highlighting
let highlightLine = $derived.by(() => {
    const hash = $page.url.hash;
    const match = hash.match(/^#line-(\d+)$/);
    return match ? parseInt(match[1], 10) : null;
});
</script>

<svelte:head>
    <title>{pathParam || "/"} · {commitHash.substring(0, 8)} · {orgName}/{repoName} – Skyr</title>
</svelte:head>

<svelte:window onmousedown={handleDeployMousedownOutside} />

<div>
  <!-- Commit message -->
  {#if commitMessage}
    <div class="bg-white border border-gray-200 rounded-lg px-4 py-3 mb-4 flex items-start gap-4">
      <div class="flex-1 min-w-0">
        <p class="font-mono text-xs text-gray-400 mb-1">{commitHash.substring(0, 8)}</p>
        <CommitMessage message={commitMessage} />
        {#if parents.length > 0}
          <p class="font-mono text-xs text-gray-400 mt-2">
            {parents.length > 1 ? "Parents:" : "Parent:"}
            {#each parents as parent, i}
              {#if i > 0}<span>, </span>{/if}
              <a
                href={commitTreeHref(orgName, repoName, parent.hash)}
                class="text-blue-600 hover:text-blue-500 transition-colors"
              >
                {parent.hash.substring(0, 8)}
              </a>
            {/each}
          </p>
        {/if}
      </div>

      <div class="deploy-dropdown relative inline-block shrink-0">
        <button
          type="button"
          class="inline-flex items-center gap-1.5 bg-white border border-gray-200 rounded px-2.5 py-1 text-xs text-gray-700 font-medium cursor-pointer hover:border-gray-300 hover:text-gray-900 transition-colors focus:outline-none focus:border-blue-500 disabled:opacity-50 disabled:cursor-not-allowed"
          disabled={!environmentsLoaded || environments.length === 0 || deployPending}
          onclick={() => {
              deployMenuOpen = !deployMenuOpen;
              deployConfirmEnv = null;
              deployError = null;
          }}
        >
          <Rocket class="w-4 h-4" />
          Deploy on...
          <ChevronDown class="w-4 h-4" />
        </button>

        {#if deployMenuOpen && deployConfirmEnv === null}
          <div
            class="absolute right-0 mt-1 z-10 bg-white border border-gray-200 rounded-lg shadow-lg py-1 w-56"
          >
            {#if environments.length === 0}
              <p class="px-3 py-2 text-sm text-gray-500">No environments.</p>
            {:else}
              {#each environments as env}
                <button
                  type="button"
                  class="w-full text-left px-3 py-1.5 text-sm text-gray-700 hover:bg-gray-100 cursor-pointer"
                  onclick={() => {
                      deployConfirmEnv = env.name;
                      deployError = null;
                  }}
                >
                  {env.name}
                </button>
              {/each}
            {/if}
          </div>
        {/if}

        {#if deployMenuOpen && deployConfirmEnv !== null}
          <div
            class="absolute right-0 mt-1 z-10 bg-white border border-gray-200 rounded-lg shadow-lg p-4 w-96"
          >
            <p class="text-sm font-medium text-gray-700 mb-2">
              Deploy on <code class="bg-gray-100 px-1 py-0.5 rounded text-xs">{deployConfirmEnv}</code>?
            </p>
            <p class="text-sm text-gray-500 mb-3">
              This will create a new deployment on
              <span class="font-medium">{deployConfirmEnv}</span>
              for commit
              <code class="bg-gray-100 px-1 py-0.5 rounded text-xs">{commitHash.substring(0, 8)}</code>,
              superseding whichever deployment is currently active. Resources
              may be created, updated, or destroyed as a result.
            </p>
            {#if deployError}
              <div class="mb-3 p-2 bg-red-50 border border-red-200 rounded text-sm text-red-600">
                {deployError}
              </div>
            {/if}
            <div class="flex gap-2 justify-end">
              <button
                type="button"
                class="px-3 py-1.5 text-sm rounded-lg border border-gray-200 text-gray-600 hover:border-gray-400 transition-colors cursor-pointer"
                onclick={() => {
                    deployConfirmEnv = null;
                    deployError = null;
                }}
              >
                Cancel
              </button>
              <button
                type="button"
                class="px-3 py-1.5 text-xs font-medium rounded bg-gray-900 text-white hover:bg-gray-800 transition-colors cursor-pointer disabled:opacity-50"
                disabled={deployPending}
                onclick={() => deployConfirmEnv && onDeploy(deployConfirmEnv)}
              >
                {deployPending ? "Deploying..." : "Confirm deploy"}
              </button>
            </div>
          </div>
        {/if}
      </div>
    </div>
  {/if}

  <!-- Breadcrumb -->
  {#if pathSegments.length > 0}
    <nav class="text-gray-400 mb-4">
      <a
        href={commitTreeHref(orgName, repoName, commitHash)}
        class="hover:text-gray-700">/</a
      >
      {#each pathSegments as segment, i}
        {#if i < pathSegments.length - 1}
          <a
            href={commitTreeHref(
              orgName,
              repoName,
              commitHash,
              pathSegments.slice(0, i + 1).join("/") + "/",
            )}
            class="hover:text-gray-700">{segment}</a
          ><span class="mx-1">/</span>
        {:else}
          <span class="text-gray-600">{segment}</span>
        {/if}
      {/each}
    </nav>
  {/if}

  {#if view.kind === "loading"}
    <Spinner />
  {:else if view.kind === "error"}
    <div class="p-4 bg-red-50 border border-red-200 rounded text-red-600">
      {view.message}
    </div>
  {:else if view.kind === "directory"}
    <DirectoryView
      {orgName}
      {repoName}
      {commitHash}
      path={pathSegments}
      entries={view.entries}
    />
  {:else if view.kind === "file"}
    <FileView
      {orgName}
      {repoName}
      {commitHash}
      path={pathSegments}
      content={view.content}
      size={view.size}
      {highlightLine}
    />
  {/if}
</div>
