<script lang="ts">
import { page } from "$app/state";
import { Menu, X } from "lucide-svelte";
import { getStdlibModules } from "$lib/stdlib";

let { children } = $props();

let mobileNavOpen = $state(false);

const stdlibRefChildren = getStdlibModules().map((m) => ({
    title: m.shortName,
    path: `/~docs/scl/stdlib-ref/${m.slug}/`,
}));

const nav = [
    { title: "Overview", path: "/~docs/" },
    { title: "Deployments", path: "/~docs/deployments/" },
    {
        title: "SCL",
        path: "/~docs/scl/",
        children: [
            { title: "Syntax", path: "/~docs/scl/syntax/" },
            { title: "Types", path: "/~docs/scl/types/" },
            { title: "Standard Library", path: "/~docs/scl/stdlib/" },
            {
                title: "Stdlib Reference",
                path: "/~docs/scl/stdlib-ref/",
                children: stdlibRefChildren,
            },
        ],
    },
];

interface NavItem {
    title: string;
    path: string;
    children?: NavItem[];
}

function isActive(path: string): boolean {
    return page.url.pathname === path;
}

function findCurrentTitle(items: NavItem[]): string | null {
    for (const item of items) {
        if (isActive(item.path)) return item.title;
        if (item.children) {
            const found = findCurrentTitle(item.children);
            if (found) return found;
        }
    }
    return null;
}

let currentTitle = $derived(findCurrentTitle(nav) ?? "Docs");
</script>

{#snippet navTree(items: NavItem[])}
    <ul class="space-y-1">
        {#each items as item}
            <li>
                <a
                    href={item.path}
                    onclick={() => (mobileNavOpen = false)}
                    class="block px-2 py-1 rounded {isActive(item.path)
                        ? 'bg-gray-100 text-gray-900 font-medium'
                        : 'text-gray-600 hover:text-gray-900'}"
                >
                    {item.title}
                </a>
                {#if item.children}
                    <div class="ml-3 mt-1">
                        {@render navTree(item.children)}
                    </div>
                {/if}
            </li>
        {/each}
    </ul>
{/snippet}

<!-- Mobile sub-header -->
<div class="md:hidden flex items-center justify-between border-b border-gray-200 bg-white px-4 h-10 sticky top-14 z-30">
    <span class="text-xs text-gray-500">{currentTitle}</span>
    <button onclick={() => (mobileNavOpen = !mobileNavOpen)} class="text-gray-600 p-1">
        {#if mobileNavOpen}
            <X size={18} />
        {:else}
            <Menu size={18} />
        {/if}
    </button>
</div>

{#if mobileNavOpen}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
        class="md:hidden fixed inset-x-0 top-14 bottom-0 z-50 bg-white flex flex-col"
        onkeydown={(e) => e.key === "Escape" && (mobileNavOpen = false)}
    >
        <div class="flex items-center justify-between border-b border-gray-200 px-4 h-10">
            <span class="text-xs text-gray-500">{currentTitle}</span>
            <button onclick={() => (mobileNavOpen = false)} class="text-gray-600 p-1">
                <X size={18} />
            </button>
        </div>
        <nav class="flex-1 p-4 text-sm overflow-y-auto">
            {@render navTree(nav)}
        </nav>
    </div>
{/if}

<div class="flex-1 bg-white flex">
    <!-- Desktop sidebar -->
    <nav class="hidden md:block w-56 shrink-0 border-r border-gray-200 p-4 text-sm sticky top-14 max-h-[calc(100vh-3.5rem)] overflow-y-auto">
        {@render navTree(nav)}
    </nav>

    <main class="flex-1 min-w-0 max-w-3xl px-8 py-6">
        {@render children()}
    </main>
</div>
