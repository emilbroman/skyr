<script lang="ts">
import { DeploymentState } from "$lib/graphql/generated";

let { state, size = "default" }: { state: DeploymentState; size?: "default" | "small" } = $props();

const styles: Record<DeploymentState, { bg: string; text: string }> = {
    [DeploymentState.Up]: {
        bg: "bg-green-50 border-green-300",
        text: "text-green-700",
    },
    [DeploymentState.Desired]: {
        bg: "bg-blue-50 border-blue-300",
        text: "text-blue-700",
    },
    [DeploymentState.Down]: {
        bg: "bg-gray-100 border-gray-300",
        text: "text-gray-500",
    },
    [DeploymentState.Undesired]: {
        bg: "bg-yellow-50 border-yellow-300",
        text: "text-yellow-700",
    },
    [DeploymentState.Lingering]: {
        bg: "bg-orange-50 border-orange-300",
        text: "text-orange-700",
    },
    [DeploymentState.Failing]: {
        bg: "bg-amber-50 border-amber-300",
        text: "text-amber-700",
    },
    [DeploymentState.Failed]: {
        bg: "bg-red-50 border-red-300",
        text: "text-red-700",
    },
};

const style = $derived(styles[state]);
const iconSize = $derived(size === "small" ? 10 : 12);
</script>

<span
    class="inline-flex items-center gap-1 rounded text-xs font-medium border {style.bg} {style.text} {size ===
    'small'
        ? 'px-1.5 py-px'
        : 'px-2 py-0.5'}"
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
    {:else if state === DeploymentState.Failing}
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
    {:else if state === DeploymentState.Failed}
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
            <line x1="4" y1="4" x2="12" y2="12" />
            <line x1="12" y1="4" x2="4" y2="12" />
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
