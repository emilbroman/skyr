<script lang="ts">
import { DeploymentState } from "$lib/graphql/generated";

let { state, bootstrapped = false, size = "default" }: { state: DeploymentState; bootstrapped?: boolean; size?: "default" | "small" } = $props();

const styles: Record<DeploymentState, { bg: string; text: string }> = {
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
};

const bootstrappedStyles = {
    bg: "bg-green-50 border-green-300",
    text: "text-green-700",
};

const style = $derived(
    state === DeploymentState.Desired && bootstrapped
        ? bootstrappedStyles
        : styles[state],
);
const iconSize = $derived(size === "small" ? 10 : 12);
const label = $derived(
    state === DeploymentState.Desired && bootstrapped
        ? "BOOTSTRAPPED"
        : state,
);
</script>

<span
    class="inline-flex items-center gap-1 rounded text-xs font-medium border {style.bg} {style.text} {size ===
    'small'
        ? 'px-1.5 py-px'
        : 'px-2 py-0.5'}"
>
    {#if state === DeploymentState.Desired && bootstrapped}
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
    {:else if state === DeploymentState.Down}
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
    {label}
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
