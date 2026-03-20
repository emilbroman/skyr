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
</script>

<span
  class="inline-flex items-center rounded font-medium border {style.bg} {style.text} {size ===
  'small'
    ? 'px-1.5 py-px text-[10px]'
    : 'px-2 py-0.5 text-xs'}"
>
  {state}
</span>
