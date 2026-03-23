<script lang="ts">
import { page } from "$app/state";

let { children } = $props();

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
</script>

<div class="flex-1 bg-white flex">
    <nav class="w-56 shrink-0 border-r border-gray-200 p-4 text-sm">
        <a href="/~docs/" class="font-bold text-gray-900 block mb-4">Docs</a>
        <ul class="space-y-1">
            {#each nav as item}
                <li>
                    <a
                        href={item.path}
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
    </nav>

    <main class="flex-1 min-w-0 max-w-3xl px-8 py-6">
        {@render children()}
    </main>
</div>
