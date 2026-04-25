<script lang="ts">
import { DeploymentState } from "$lib/graphql/generated";

let {
    state,
    bootstrapped = false,
    volatile = false,
    size = "default",
}: {
    state: DeploymentState;
    bootstrapped?: boolean;
    volatile?: boolean;
    size?: "default" | "small";
} = $props();

const green = { bg: "bg-green-50 border-green-300", text: "text-green-700" };
const yellow = { bg: "bg-yellow-50 border-yellow-300", text: "text-yellow-700" };
const gray = { bg: "bg-gray-100 border-gray-300", text: "text-gray-500" };

let isUp = $derived(
    bootstrapped && (state === DeploymentState.Desired || state === DeploymentState.Lingering),
);

type Icon = "check" | "spinner-fast" | "spinner-slow" | "spinner-fast-ccw" | "exclamation";

let style = $derived(isUp ? green : state === DeploymentState.Down ? gray : yellow);
let icon: Icon = $derived(
    isUp
        ? volatile
            ? "spinner-slow"
            : "check"
        : state === DeploymentState.Down
          ? "check"
          : state === DeploymentState.Undesired
            ? "spinner-fast-ccw"
            : "spinner-fast",
);
let label = $derived(
    isUp
        ? "UP"
        : state === DeploymentState.Down
          ? "DOWN"
          : state === DeploymentState.Undesired
            ? "DESTROYING"
            : "DEPLOYING",
);

const iconSize = $derived(size === "small" ? 10 : 12);
</script>

<span
    class="inline-flex items-center gap-1 rounded text-xs font-medium border {style.bg} {style.text} {size ===
    'small'
        ? 'px-1.5 py-px'
        : 'px-2 py-0.5'}"
>
    {#if icon === "check"}
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
    {:else if icon === "spinner-fast"}
        <svg
            class="spinner-fast"
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
    {:else if icon === "spinner-slow"}
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
    {:else if icon === "spinner-fast-ccw"}
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
    {/if}
    {label}
</span>

<style>
    .spinner-fast {
        animation: spin 2s linear infinite;
    }

    .spinner-slow {
        animation: spin 12s linear infinite;
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
