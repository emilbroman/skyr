<script lang="ts">
import { replaceState } from "$app/navigation";
import { page } from "$app/stores";
import DirectoryView from "$lib/components/DirectoryView.svelte";
import FileView from "$lib/components/FileView.svelte";
import { query } from "$lib/graphql/client";
import { CommitRootTreeDocument, CommitTreeEntryDocument } from "$lib/graphql/generated";
import { commitTreeHref } from "$lib/paths";

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
    | { kind: "directory"; entries: TreeEntry[]; commitMessage: string }
    | {
          kind: "file";
          content: string | null;
          size: number;
          commitMessage: string;
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
            };
        } else {
            view = {
                kind: "file",
                content: entry.content ?? null,
                size: entry.size,
                commitMessage: commit.message,
            };
        }
    } catch (e) {
        view = {
            kind: "error",
            message: e instanceof Error ? e.message : "Failed to load path",
        };
    }
}

// Trailing slash normalization: directories get trailing slash, files lose it.
// Uses replaceState to update the URL without triggering a navigation cycle.
$effect(() => {
    const currentUrl = $page.url.pathname;
    const hasTrailingSlash = currentUrl.endsWith("/");
    if (view.kind === "directory" && !hasTrailingSlash) {
        replaceState(`${currentUrl}/`, {});
    } else if (view.kind === "file" && hasTrailingSlash) {
        replaceState(currentUrl.replace(/\/+$/, ""), {});
    }
});

// Load data when path changes
$effect(() => {
    if (isRoot) {
        loadRoot();
    } else {
        loadPath(pathSegments.join("/"));
    }
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

<div>
  <!-- Breadcrumb -->
  <nav class="text-sm text-gray-500 mb-4">
    <a
      href={commitTreeHref(orgName, repoName, commitHash)}
      class="hover:text-gray-300 font-mono text-xs"
      >{commitHash.substring(0, 8)}</a
    >
    {#each pathSegments as segment, i}
      <span class="mx-2">/</span>
      {#if i < pathSegments.length - 1}
        <a
          href={commitTreeHref(
            orgName,
            repoName,
            commitHash,
            pathSegments.slice(0, i + 1).join("/") + "/",
          )}
          class="hover:text-gray-300">{segment}</a
        >
      {:else}
        <span class="text-gray-300">{segment}</span>
      {/if}
    {/each}
    {#if commitMessage}
      <span class="ml-3 text-gray-600">&mdash; {commitMessage}</span>
    {/if}
  </nav>

  {#if view.kind === "loading"}
    <p class="text-gray-400">Loading...</p>
  {:else if view.kind === "error"}
    <div class="p-4 bg-red-900/20 border border-red-800 rounded text-red-300">
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
