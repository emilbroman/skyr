<script lang="ts">
import { page } from "$app/state";
import { Menu, X } from "lucide-svelte";

let { children } = $props();

let mobileNavOpen = $state(false);

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
        ],
    },
];

function isActive(path: string): boolean {
    return page.url.pathname === path;
}

function findCurrentTitle(): string {
    for (const item of nav) {
        if (isActive(item.path)) return item.title;
        if (item.children) {
            for (const child of item.children) {
                if (isActive(child.path)) return child.title;
            }
        }
    }
    return "Docs";
}

let currentTitle = $derived(findCurrentTitle());
</script>

{#snippet navLinks()}
    <ul class="space-y-1">
        {#each nav as item}
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
                    <ul class="ml-3 mt-1 space-y-1">
                        {#each item.children as child}
                            <li>
                                <a
                                    href={child.path}
                                    onclick={() => (mobileNavOpen = false)}
                                    class="block px-2 py-1 rounded {isActive(child.path)
                                        ? 'bg-gray-100 text-gray-900 font-medium'
                                        : 'text-gray-600 hover:text-gray-900'}"
                                >
                                    {child.title}
                                </a>
                            </li>
                        {/each}
                    </ul>
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
            {@render navLinks()}
        </nav>
    </div>
{/if}

<div class="flex-1 bg-white flex">
    <!-- Desktop sidebar -->
    <nav class="hidden md:block w-56 shrink-0 border-r border-gray-200 p-4 text-sm">
        {@render navLinks()}
    </nav>

    <main class="flex-1 min-w-0 max-w-3xl px-8 py-6">
        {@render children()}
    </main>
</div>
