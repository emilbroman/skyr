<script lang="ts">
import { Box, RefreshCw } from "lucide-svelte";
import { deploymentHref, resourceHref } from "$lib/paths";

type Entity =
    | { __typename: "Deployment"; id: string; shortId: string }
    | { __typename: "Resource"; type: string; name: string };

let {
    entity,
    org,
    repo,
    env,
    class: className = "",
}: {
    entity: Entity;
    org: string;
    repo: string;
    env: string;
    class?: string;
} = $props();
</script>

{#if entity.__typename === "Deployment"}
    <a
        class="text-gray-900 hover:text-gray-600 font-mono text-xs {className}"
        href={deploymentHref(org, repo, env, entity.id)}
    >
        <RefreshCw class="w-3.5 h-3.5 inline-block align-text-bottom -mr-1.5" />
        {entity.shortId}
    </a>
{:else if entity.__typename === "Resource"}
    <a
        class="text-gray-900 hover:text-gray-600 font-mono text-xs {className}"
        href={resourceHref(org, repo, env, `${entity.type}:${entity.name}`)}
    >
        <Box class="w-3.5 h-3.5 inline-block align-text-bottom -mr-1.5" />
        {entity.type.replace(/^[^.]*\./, "")}
        {entity.name}
    </a>
{/if}
