<script lang="ts">
import { DeploymentState } from "$lib/graphql/generated";

let { state, size = "default" }: { state: DeploymentState; size?: "default" | "small" } = $props();

const styles: Record<DeploymentState, { bg: string; text: string }> = {
    [DeploymentState.Up]: {
        bg: "bg-green-900/40 border-green-700",
        text: "text-green-300",
    },
    [DeploymentState.Desired]: {
        bg: "bg-blue-900/40 border-blue-700",
        text: "text-blue-300",
    },
    [DeploymentState.Down]: {
        bg: "bg-gray-800 border-gray-600",
        text: "text-gray-400",
    },
    [DeploymentState.Undesired]: {
        bg: "bg-yellow-900/40 border-yellow-700",
        text: "text-yellow-300",
    },
    [DeploymentState.Lingering]: {
        bg: "bg-orange-900/40 border-orange-700",
        text: "text-orange-300",
    },
};

const style = $derived(styles[state]);
const iconSize = $derived(size === "small" ? 10 : 12);
</script>

<span
    class="inline-flex items-center gap-1 rounded font-medium border {style.bg} {style.text} {size ===
    'small'
        ? 'px-1.5 py-px text-[10px]'
        : 'px-2 py-0.5 text-xs'}"
>
    {#if state === DeploymentState.Up || state === DeploymentState.Down}
        <svg
            width={iconSize}
            height={iconSize}
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
            stroke-linejoin="round"
        >
            <polyline points="3,8.5 6.5,12 13,4" />
        </svg>
    {:else if state === DeploymentState.Desired}
        <svg
            class="spinner-slow"
            width={iconSize}
            height={iconSize}
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
        >
            <path d="M8 1.5a6.5 6.5 0 1 1-6.5 6.5" />
        </svg>
    {:else if state === DeploymentState.Undesired}
        <svg
            class="spinner-fast-ccw"
            width={iconSize}
            height={iconSize}
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
        >
            <path d="M8 1.5a6.5 6.5 0 1 1-6.5 6.5" />
        </svg>
    {:else if state === DeploymentState.Lingering}
        <svg
            class="spinner-slow"
            width={iconSize}
            height={iconSize}
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
        >
            <path d="M8 1.5a6.5 6.5 0 1 1-6.5 6.5" />
        </svg>
    {/if}
    {state}
</span>

<style>
    .spinner-slow {
        animation: spin 4s linear infinite;
    }

    .spinner-fast-ccw {
        animation: spin-ccw 2s linear infinite;
    }

    @keyframes spin {
        from {
            transform: rotate(0deg);
        }
        to {
            transform: rotate(360deg);
        }
    }

    @keyframes spin-ccw {
        from {
            transform: rotate(0deg);
        }
        to {
            transform: rotate(-360deg);
        }
    }
</style>
