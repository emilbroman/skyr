<script lang="ts">
import { HealthStatus } from "$lib/graphql/generated";

let {
    health,
    openIncidentCount = 0,
    size = "default",
    showLabel = true,
}: {
    health: HealthStatus;
    openIncidentCount?: number;
    size?: "default" | "small";
    showLabel?: boolean;
} = $props();

const green = { bg: "bg-green-50 border-green-300", text: "text-green-700" };
const yellow = { bg: "bg-yellow-50 border-yellow-300", text: "text-yellow-700" };
const red = { bg: "bg-red-50 border-red-300", text: "text-red-700" };

let style = $derived(
    health === HealthStatus.Healthy ? green : health === HealthStatus.Down ? red : yellow,
);

let dotColor = $derived(
    health === HealthStatus.Healthy
        ? "bg-green-500"
        : health === HealthStatus.Down
          ? "bg-red-500"
          : "bg-yellow-500",
);

let label = $derived.by(() => {
    if (health === HealthStatus.Healthy) return "HEALTHY";
    if (health === HealthStatus.Down) return "DOWN";
    return "DEGRADED";
});
</script>

{#if showLabel}
    <span
        class="inline-flex items-center gap-1 rounded text-xs font-medium border {style.bg} {style.text} {size ===
        'small'
            ? 'px-1.5 py-px'
            : 'px-2 py-0.5'}"
        title={openIncidentCount > 0
            ? `${openIncidentCount} open incident${openIncidentCount === 1 ? "" : "s"}`
            : undefined}
    >
        <span
            class="inline-block rounded-full {dotColor} {size === 'small' ? 'w-1.5 h-1.5' : 'w-2 h-2'}"
        ></span>
        {label}{#if openIncidentCount > 0}
            <span class="opacity-70">·{openIncidentCount}</span>
        {/if}
    </span>
{:else}
    <span
        class="inline-block rounded-full {dotColor} {size === 'small' ? 'w-1.5 h-1.5' : 'w-2 h-2'}"
        title={`${label}${openIncidentCount > 0 ? ` · ${openIncidentCount} open incident${openIncidentCount === 1 ? "" : "s"}` : ""}`}
    ></span>
{/if}
