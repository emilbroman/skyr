<script lang="ts">
let { data } = $props();
</script>

<svelte:head>
    <title>Standard Library Reference – Skyr Docs</title>
</svelte:head>

<article>
    <h1 class="text-base font-semibold text-gray-900 mb-4 pb-3 border-b border-gray-200">Standard Library Reference</h1>

    <div class="bg-white border border-gray-200 rounded overflow-hidden divide-y divide-gray-100">
        {#each data.modules as mod}
            <a
                href="/~docs/scl/stdlib-ref/{mod.slug}/"
                class="block px-4 py-2.5 text-xs hover:bg-gray-50 transition-colors group"
            >
                <code class="text-gray-800 group-hover:text-blue-600">{mod.name}</code>
            </a>
        {/each}
    </div>
</article>
