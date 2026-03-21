<script lang="ts">
import { ResourceMarker } from "$lib/graphql/generated";
import { resourceHref } from "$lib/paths";

type ResourceInput = {
    type: string;
    name: string;
    markers: ResourceMarker[];
    owner?: { id: string } | null;
    dependencies: { type: string; name: string }[];
};

let {
    resources,
    org,
    repo,
    env,
}: {
    resources: ResourceInput[];
    org: string;
    repo: string;
    env: string;
} = $props();

const NODE_W = 200;
const NODE_H = 72;
const LAYER_GAP = 120;
const NODE_GAP = 24;

type DagNode = {
    id: string;
    type: string;
    name: string;
    markers: ResourceMarker[];
    isShadow: boolean;
    layer: number;
    x: number;
    y: number;
    href?: string;
};

type DagEdge = {
    from: string;
    to: string;
};

let dag = $derived.by(() => {
    const nodeMap = new Map<string, DagNode>();
    const edges: DagEdge[] = [];

    for (const r of resources) {
        const id = `${r.type}:${r.name}`;
        nodeMap.set(id, {
            id,
            type: r.type,
            name: r.name,
            markers: r.markers,
            isShadow: false,
            layer: 0,
            x: 0,
            y: 0,
            href: resourceHref(org, repo, env, id),
        });
    }

    for (const r of resources) {
        const id = `${r.type}:${r.name}`;
        for (const dep of r.dependencies) {
            const depId = `${dep.type}:${dep.name}`;
            if (!nodeMap.has(depId)) {
                nodeMap.set(depId, {
                    id: depId,
                    type: dep.type,
                    name: dep.name,
                    markers: [],
                    isShadow: true,
                    layer: 0,
                    x: 0,
                    y: 0,
                });
            }
            edges.push({ from: depId, to: id });
        }
    }

    // Build dependency lookup (node -> its dependencies)
    const depsOf = new Map<string, string[]>();
    for (const e of edges) {
        if (!depsOf.has(e.to)) depsOf.set(e.to, []);
        depsOf.get(e.to)!.push(e.from);
    }

    // Assign layers via longest-path from roots
    const layerCache = new Map<string, number>();
    const visiting = new Set<string>();

    function computeLayer(nodeId: string): number {
        if (layerCache.has(nodeId)) return layerCache.get(nodeId)!;
        if (visiting.has(nodeId)) return 0;
        visiting.add(nodeId);
        const deps = depsOf.get(nodeId) ?? [];
        const layer = deps.length === 0 ? 0 : Math.max(...deps.map(computeLayer)) + 1;
        visiting.delete(nodeId);
        layerCache.set(nodeId, layer);
        return layer;
    }

    const nodes = [...nodeMap.values()];
    for (const node of nodes) {
        node.layer = computeLayer(node.id);
    }

    // Group by layer
    const layers = new Map<number, DagNode[]>();
    for (const node of nodes) {
        if (!layers.has(node.layer)) layers.set(node.layer, []);
        layers.get(node.layer)!.push(node);
    }

    const maxLayer = Math.max(...nodes.map((n) => n.layer), 0);

    // Order within layers: layer 0 alphabetically, rest by barycenter
    const orderMap = new Map<string, number>();
    const layer0 = layers.get(0) ?? [];
    layer0.sort((a, b) => a.id.localeCompare(b.id));
    layer0.forEach((n, i) => {
        orderMap.set(n.id, i);
    });

    for (let l = 1; l <= maxLayer; l++) {
        const layerNodes = layers.get(l) ?? [];
        const bary = new Map<string, number>();
        for (const node of layerNodes) {
            const deps = depsOf.get(node.id) ?? [];
            if (deps.length > 0) {
                bary.set(
                    node.id,
                    deps.reduce((sum, d) => sum + (orderMap.get(d) ?? 0), 0) / deps.length,
                );
            } else {
                bary.set(node.id, 0);
            }
        }
        layerNodes.sort((a, b) => (bary.get(a.id) ?? 0) - (bary.get(b.id) ?? 0));
        layerNodes.forEach((n, i) => {
            orderMap.set(n.id, i);
        });
    }

    // Position nodes, centering each layer vertically
    const maxLayerSize = Math.max(...[...layers.values()].map((l) => l.length), 1);
    const totalMaxHeight = maxLayerSize * (NODE_H + NODE_GAP) - NODE_GAP;

    for (const [, layerNodes] of layers) {
        const layerHeight = layerNodes.length * (NODE_H + NODE_GAP) - NODE_GAP;
        const offset = (totalMaxHeight - layerHeight) / 2;
        layerNodes.forEach((node, i) => {
            node.x = node.layer * (NODE_W + LAYER_GAP);
            node.y = offset + i * (NODE_H + NODE_GAP);
        });
    }

    return { nodes, edges, nodeMap };
});

function edgePath(edge: DagEdge): string {
    const from = dag.nodeMap.get(edge.from);
    const to = dag.nodeMap.get(edge.to);
    if (!from || !to) return "";
    const x1 = from.x + NODE_W;
    const y1 = from.y + NODE_H / 2;
    const x2 = to.x;
    const y2 = to.y + NODE_H / 2;
    const dx = Math.abs(x2 - x1) * 0.4;
    return `M ${x1} ${y1} C ${x1 + dx} ${y1}, ${x2 - dx} ${y2}, ${x2} ${y2}`;
}

// Pan/zoom
let svgEl: SVGSVGElement | undefined = $state();
let panX = $state(0);
let panY = $state(0);
let zoom = $state(1);
let isPanning = $state(false);
let panStartX = $state(0);
let panStartY = $state(0);
let hoveredNode = $state<string | null>(null);
let lastNodeCount = $state(0);

function fitToView() {
    if (!svgEl || dag.nodes.length === 0) return;
    const rect = svgEl.getBoundingClientRect();
    const pad = 60;

    const minX = Math.min(...dag.nodes.map((n) => n.x));
    const maxX = Math.max(...dag.nodes.map((n) => n.x + NODE_W));
    const minY = Math.min(...dag.nodes.map((n) => n.y));
    const maxY = Math.max(...dag.nodes.map((n) => n.y + NODE_H));

    const graphW = maxX - minX || 1;
    const graphH = maxY - minY || 1;

    const scaleX = (rect.width - pad * 2) / graphW;
    const scaleY = (rect.height - pad * 2) / graphH;
    zoom = Math.min(scaleX, scaleY, 1.5);

    panX = (rect.width - graphW * zoom) / 2 - minX * zoom;
    panY = (rect.height - graphH * zoom) / 2 - minY * zoom;
}

$effect(() => {
    const count = dag.nodes.length;
    if (count > 0 && svgEl && count !== lastNodeCount) {
        fitToView();
        lastNodeCount = count;
    }
});

function onMouseDown(e: MouseEvent) {
    if (e.button !== 0) return;
    isPanning = true;
    panStartX = e.clientX - panX;
    panStartY = e.clientY - panY;
}

function onMouseMove(e: MouseEvent) {
    if (!isPanning) return;
    panX = e.clientX - panStartX;
    panY = e.clientY - panStartY;
}

function onMouseUp() {
    isPanning = false;
}

function zoomAt(cx: number, cy: number, factor: number) {
    const newZoom = Math.max(0.1, Math.min(3, zoom * factor));
    panX = cx - (cx - panX) * (newZoom / zoom);
    panY = cy - (cy - panY) * (newZoom / zoom);
    zoom = newZoom;
}

function onWheel(e: WheelEvent) {
    e.preventDefault();
    const rect = svgEl!.getBoundingClientRect();
    zoomAt(e.clientX - rect.left, e.clientY - rect.top, e.deltaY > 0 ? 0.9 : 1.1);
}

function zoomInCenter() {
    if (!svgEl) return;
    const rect = svgEl.getBoundingClientRect();
    zoomAt(rect.width / 2, rect.height / 2, 1.25);
}

function zoomOutCenter() {
    if (!svgEl) return;
    const rect = svgEl.getBoundingClientRect();
    zoomAt(rect.width / 2, rect.height / 2, 0.8);
}

function isEdgeHighlighted(edge: DagEdge): boolean {
    return hoveredNode !== null && (edge.from === hoveredNode || edge.to === hoveredNode);
}

function typeParts(type: string): { prefix: string; last: string } {
    const parts = type.split(".");
    if (parts.length > 1) {
        return {
            prefix: `${parts.slice(0, -1).join(".")}.`,
            last: parts[parts.length - 1],
        };
    }
    return { prefix: "", last: type };
}
</script>

<svelte:window onmousemove={onMouseMove} onmouseup={onMouseUp} />

<div
  class="relative w-full rounded-lg border border-gray-200 overflow-hidden"
  style="height: calc(100vh - 240px); min-height: 400px; background-color: #f9fafb; background-image: radial-gradient(circle, rgba(0,0,0,0.06) 1px, transparent 1px); background-size: 24px 24px;"
>
  {#if dag.nodes.length === 0}
    <div class="flex items-center justify-center h-full text-gray-500">
      No resources in this environment.
    </div>
  {:else}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <svg
      bind:this={svgEl}
      class="w-full h-full"
      onmousedown={onMouseDown}
      onwheel={onWheel}
      style="cursor: {isPanning ? 'grabbing' : 'grab'};"
    >
      <defs>
        <marker
          id="arrow"
          viewBox="0 0 8 8"
          refX="8"
          refY="4"
          markerWidth="8"
          markerHeight="8"
          orient="auto"
        >
          <path
            d="M 0 1 L 7 4 L 0 7"
            fill="none"
            stroke="#9ca3af"
            stroke-width="1.5"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </marker>
        <marker
          id="arrow-hl"
          viewBox="0 0 8 8"
          refX="8"
          refY="4"
          markerWidth="8"
          markerHeight="8"
          orient="auto"
        >
          <path
            d="M 0 1 L 7 4 L 0 7"
            fill="none"
            stroke="#ea580c"
            stroke-width="1.5"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </marker>
      </defs>

      <g transform="translate({panX}, {panY}) scale({zoom})">
        {#each dag.edges as edge}
          <path
            d={edgePath(edge)}
            fill="none"
            stroke={isEdgeHighlighted(edge) ? "#ea580c" : "#d1d5db"}
            stroke-width={isEdgeHighlighted(edge) ? 2 : 1.5}
            opacity={hoveredNode && !isEdgeHighlighted(edge) ? 0.15 : 1}
            marker-end="url(#{isEdgeHighlighted(edge) ? 'arrow-hl' : 'arrow'})"
          />
        {/each}

        {#each dag.nodes as node}
          {@const tp = typeParts(node.type)}
          <foreignObject x={node.x} y={node.y} width={NODE_W} height={NODE_H}>
            <div xmlns="http://www.w3.org/1999/xhtml" class="w-full h-full">
              {#if node.href}
                <a
                  href={node.href}
                  class="flex flex-col justify-center w-full h-full rounded-lg border px-3 py-2 transition-all duration-150
										{node.isShadow
                    ? 'border-dashed border-amber-300/60 bg-white/60'
                    : 'border-gray-300 bg-white hover:border-orange-500'}
										{hoveredNode === node.id
                    ? node.isShadow
                      ? 'border-amber-500 shadow-lg shadow-amber-500/10'
                      : 'border-orange-500 shadow-lg shadow-orange-500/10'
                    : ''}
										{hoveredNode && hoveredNode !== node.id ? 'opacity-40' : ''}"
                  onmouseenter={() => (hoveredNode = node.id)}
                  onmouseleave={() => (hoveredNode = null)}
                >
                  <div
                    class="truncate {node.isShadow
                      ? 'text-gray-400'
                      : 'text-orange-500/70'}"
                  >
                    {#if tp.prefix}<span>{tp.prefix}</span>{/if}
                    <span
                      class={node.isShadow
                        ? "text-gray-500"
                        : "text-orange-500"}>{tp.last}</span
                    >
                  </div>
                  <div class="flex items-center gap-1.5 mt-0.5">
                    <span
                      class="truncate {node.isShadow
                        ? 'text-gray-400'
                        : 'text-gray-600'}">{node.name}</span
                    >
                    {#each node.markers as marker}
                      <span
                        class="px-1 py-px rounded border shrink-0 {marker ===
                        ResourceMarker.Volatile
                          ? 'border-yellow-300 text-yellow-700'
                          : 'border-blue-300 text-blue-700'}"
                      >
                        {marker}
                      </span>
                    {/each}
                  </div>
                  {#if node.isShadow}
                    <div class="text-amber-500/70 mt-0.5">
                      External
                    </div>
                  {/if}
                </a>
              {:else}
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <div
                  class="flex flex-col justify-center w-full h-full rounded-lg border border-dashed px-3 py-2 transition-all duration-150
										border-amber-300/60 bg-white/60
										{hoveredNode === node.id
                    ? 'border-amber-500 shadow-lg shadow-amber-500/10'
                    : ''}
										{hoveredNode && hoveredNode !== node.id ? 'opacity-40' : ''}"
                  onmouseenter={() => (hoveredNode = node.id)}
                  onmouseleave={() => (hoveredNode = null)}
                >
                  <div class="text-gray-400 truncate">
                    {#if tp.prefix}<span>{tp.prefix}</span>{/if}
                    <span class="text-gray-500">{tp.last}</span>
                  </div>
                  <div class="flex items-center gap-1.5 mt-0.5">
                    <span class="text-gray-400 truncate"
                      >{node.name}</span
                    >
                  </div>
                  <div class="text-amber-500/70 mt-0.5">
                    External
                  </div>
                </div>
              {/if}
            </div>
          </foreignObject>
        {/each}
      </g>
    </svg>

    <div class="absolute bottom-3 right-3 flex gap-1">
      <button
        class="px-2.5 py-1 bg-white/80 backdrop-blur border border-gray-300 rounded text-gray-600 hover:bg-gray-200 transition-colors"
        onclick={fitToView}
      >
        Fit
      </button>
      <button
        class="px-2.5 py-1 bg-white/80 backdrop-blur border border-gray-300 rounded text-gray-600 hover:bg-gray-200 transition-colors"
        onclick={zoomInCenter}
      >
        +
      </button>
      <button
        class="px-2.5 py-1 bg-white/80 backdrop-blur border border-gray-300 rounded text-gray-600 hover:bg-gray-200 transition-colors"
        onclick={zoomOutCenter}
      >
        &minus;
      </button>
    </div>
  {/if}
</div>
