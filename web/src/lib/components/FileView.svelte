<script lang="ts">
import { marked } from "marked";
import type { ThemedToken } from "shiki";
import { highlight } from "$lib/highlight";

type SourceFrame = {
    moduleId: string;
    span: string;
    name: string;
};

type ResourceInfo = {
    type: string;
    name: string;
    sourceTrace?: SourceFrame[];
};

type Props = {
    orgName: string;
    repoName: string;
    commitHash: string;
    /** Path segments to this file */
    path: string[];
    content: string | null;
    size: number;
    resources?: ResourceInfo[];
    highlightLine?: number | null;
};

let {
    orgName,
    repoName,
    commitHash,
    path,
    content,
    size,
    resources = [],
    highlightLine = null,
}: Props = $props();

let highlightedLines = $state<ThemedToken[][] | null>(null);
let highlightBg = $state<string>("#ffffff");

let filename = $derived(path[path.length - 1] ?? "");
let isMarkdown = $derived(/\.md$/i.test(filename));
let showSource = $state(false);
let renderedMarkdown = $derived(
    isMarkdown && content != null ? (marked.parse(content, { async: false }) as string) : "",
);

$effect(() => {
    highlightedLines = null;
    if (content != null && filename) {
        highlight(content, filename)
            .then((result) => {
                highlightedLines = result.lines;
                highlightBg = result.bg;
            })
            .catch(() => {
                // fall back to plain text
            });
    }
});

function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/**
 * Strip the package prefix from a moduleId.
 * Module IDs are fully qualified: "org/repo/Module" where "org/repo" is the
 * 2-segment package prefix. The file path within the repo is everything after.
 */
function moduleIdToLocalPath(moduleId: string): string {
    const segments = moduleId.split("/");
    return segments.length > 2 ? segments.slice(2).join("/") : moduleId;
}

function parseSpanStartLine(span: string): number {
    const startPart = span.split(",")[0];
    const line = parseInt(startPart.split(":")[0], 10);
    return Number.isNaN(line) ? 1 : line;
}

/**
 * Build a map from line number to resource labels for the currently viewed file.
 */
let resourceInlays = $derived.by(() => {
    if (!content || !resources.length) return new Map<number, string[]>();

    const currentFile = path.join("/");
    const modulePathForFile = currentFile.replace(/\.scl$/, "");

    const inlays = new Map<number, string[]>();
    for (const resource of resources) {
        if (!resource.sourceTrace?.length) continue;
        const frame = resource.sourceTrace[0];
        if (moduleIdToLocalPath(frame.moduleId) !== modulePathForFile) continue;
        const line = parseSpanStartLine(frame.span);
        const label = `${resource.type}/${resource.name}`;
        const existing = inlays.get(line);
        if (existing) {
            existing.push(label);
        } else {
            inlays.set(line, [label]);
        }
    }
    return inlays;
});

// Scroll to highlighted line after render
$effect(() => {
    if (highlightLine && highlightedLines) {
        const el = document.getElementById(`line-${highlightLine}`);
        el?.scrollIntoView({ behavior: "smooth", block: "center" });
    }
});
</script>

<div class="bg-white border border-gray-200 rounded-lg overflow-hidden">
  <div
    class="flex items-center justify-between px-4 py-2 border-b border-gray-200 bg-gray-50"
  >
    <span class="text-gray-500">{formatSize(size)}</span>
    {#if isMarkdown && content != null}
      <button
        class="transition-colors {showSource
          ? 'text-gray-500 hover:text-gray-800'
          : 'text-orange-600 hover:text-orange-500'}"
        onclick={() => (showSource = !showSource)}
      >
        {showSource ? "Preview" : "Source"}
      </button>
    {/if}
  </div>
  {#snippet resourceInlay(items: string[])}
    {#if items.length === 1}
      <span class="ml-4 text-orange-500/70 font-sans select-none"
        >{items[0]}</span
      >
    {:else}
      <span
        class="ml-4 relative inline-block font-sans select-none group/inlay"
      >
        <span class="text-orange-500/70 cursor-default"
          >{items.length} resources</span
        >
        <div
          class="hidden group-hover/inlay:block absolute left-0 top-full z-10 mt-1 py-1 px-2 bg-gray-100 border border-gray-300 rounded shadow-lg whitespace-nowrap"
        >
          {#each items as item}
            <div class="text-orange-500 leading-5">{item}</div>
          {/each}
        </div>
      </span>
    {/if}
  {/snippet}
  {#if content != null && isMarkdown && !showSource}
    <div class="p-6 prose prose-sm max-w-none">
      {@html renderedMarkdown}
    </div>
  {:else if content != null}
    <div class="overflow-x-auto" style="background:{highlightBg}">
      <table class="w-full font-mono text-xs leading-5 border-collapse">
        <tbody>
          {#if highlightedLines}
            {#each highlightedLines as tokens, i}
              {@const lineNum = i + 1}
              {@const inlay = resourceInlays.get(lineNum)}
              <tr
                id="line-{lineNum}"
                class="hover:bg-gray-100 {highlightLine === lineNum
                  ? 'bg-orange-100'
                  : ''}"
              >
                <td
                  class="px-4 py-0 text-right text-gray-400 select-none align-top w-12 whitespace-nowrap"
                  >{lineNum}</td
                >
                <td class="px-4 py-0 whitespace-pre"
                  >{#each tokens as token}<span
                      style="color:{token.color ??
                        ''};font-style:{token.fontStyle === 1
                        ? 'italic'
                        : 'normal'}">{token.content}</span
                    >{/each}{#if inlay}{@render resourceInlay(inlay)}{/if}</td
                >
              </tr>
            {/each}
          {:else}
            {#each content.split("\n") as line, i}
              {@const lineNum = i + 1}
              {@const inlay = resourceInlays.get(lineNum)}
              <tr
                id="line-{lineNum}"
                class="hover:bg-gray-100 {highlightLine === lineNum
                  ? 'bg-orange-100'
                  : ''}"
              >
                <td
                  class="px-4 py-0 text-right text-gray-400 select-none align-top w-12 whitespace-nowrap"
                  >{lineNum}</td
                >
                <td class="px-4 py-0 whitespace-pre text-gray-600"
                  >{line}{#if inlay}{@render resourceInlay(inlay)}{/if}</td
                >
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </div>
  {:else}
    <div class="p-8 text-center text-gray-400">
      Binary file ({formatSize(size)})
    </div>
  {/if}
</div>
