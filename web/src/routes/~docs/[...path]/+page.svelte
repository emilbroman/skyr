<script lang="ts">
let { data } = $props();
</script>

<svelte:head>
    <title>{data.title} – Skyr Docs</title>
</svelte:head>

<article class="prose prose-sm max-w-none prose-headings:font-semibold prose-h1:text-xl prose-h1:mt-0 prose-h2:text-base prose-h3:text-sm prose-pre:bg-gray-50 prose-pre:border prose-pre:border-gray-200 prose-pre:text-gray-800 prose-code:text-gray-800 prose-code:before:content-none prose-code:after:content-none prose-a:text-blue-600 prose-a:no-underline hover:prose-a:underline">
    {@html data.html}
</article>
