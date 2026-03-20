<script lang="ts">
import "../app.css";
import { QueryClient, QueryClientProvider } from "@tanstack/svelte-query";
import { onDestroy, onMount } from "svelte";
import { startExpiryWatch, stopExpiryWatch } from "$lib/stores/auth";

let { children } = $props();

const queryClient = new QueryClient({
    defaultOptions: {
        queries: {
            staleTime: 30_000,
            refetchOnWindowFocus: false,
        },
    },
});

onMount(() => {
    startExpiryWatch();
});

onDestroy(() => {
    stopExpiryWatch();
});
</script>

<QueryClientProvider client={queryClient}>
  {@render children()}
</QueryClientProvider>
