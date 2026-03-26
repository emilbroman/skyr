export interface DiagnosticInfo {
    line: number;
    character: number;
    end_line: number;
    end_character: number;
    message: string;
    severity: "error" | "warning";
}

export interface HoverInfo {
    type: string;
}

export interface CompletionItem {
    label: string;
    kind: "variable" | "field";
}

export interface LocationInfo {
    line: number;
    character: number;
    end_line: number;
    end_character: number;
}

type PendingRequest = {
    resolve: (value: unknown) => void;
    reject: (reason: unknown) => void;
};

export class SclWorker {
    private worker: Worker;
    private nextId = 0;
    private pending = new Map<number, PendingRequest>();

    constructor() {
        this.worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
        this.worker.onmessage = (e) => {
            const { id, result, error } = e.data;
            const p = this.pending.get(id);
            if (!p) return;
            this.pending.delete(id);
            if (error) {
                p.reject(new Error(error));
            } else {
                p.resolve(result);
            }
        };
    }

    private request(msg: Record<string, unknown>): Promise<unknown> {
        const id = this.nextId++;
        return new Promise((resolve, reject) => {
            this.pending.set(id, { resolve, reject });
            this.worker.postMessage({ ...msg, id });
        });
    }

    async analyze(source: string): Promise<DiagnosticInfo[]> {
        return (await this.request({ type: "analyze", source })) as DiagnosticInfo[];
    }

    async hover(source: string, line: number, col: number): Promise<HoverInfo | null> {
        return (await this.request({ type: "hover", source, line, col })) as HoverInfo | null;
    }

    async completions(source: string, line: number, col: number): Promise<CompletionItem[]> {
        return (await this.request({ type: "completions", source, line, col })) as CompletionItem[];
    }

    async gotoDefinition(source: string, line: number, col: number): Promise<LocationInfo | null> {
        return (await this.request({
            type: "gotoDefinition",
            source,
            line,
            col,
        })) as LocationInfo | null;
    }

    async format(source: string): Promise<string | null> {
        return (await this.request({ type: "format", source })) as string | null;
    }

    dispose() {
        this.worker.terminate();
    }
}
