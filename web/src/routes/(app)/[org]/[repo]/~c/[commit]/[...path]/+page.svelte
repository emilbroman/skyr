<script lang="ts">
import { replaceState } from "$app/navigation";
import { page } from "$app/stores";
import { untrack } from "svelte";
import DirectoryView from "$lib/components/DirectoryView.svelte";
import Spinner from "$lib/components/Spinner.svelte";
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

// Load data when path changes.
// Uses untrack for the load calls so that setting `view` doesn't
// re-subscribe this effect to anything new.
$effect(() => {
    // Subscribe to the path-derived reactives:
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

<div>
  <!-- Commit message -->
  {#if commitMessage}
    <div class="bg-white border border-gray-200 rounded-lg px-4 py-3 mb-4">
      <p class="font-mono text-xs text-gray-400 mb-1">{commitHash.substring(0, 8)}</p>
      <p class="text-gray-700 whitespace-pre-line">{commitMessage}</p>
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
