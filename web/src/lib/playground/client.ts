export interface DiagnosticInfo {
    file: string;
    line: number;
    character: number;
    end_line: number;
    end_character: number;
    message: string;
    severity: "error" | "warning";
}

export interface HoverInfo {
    type: string;
    description?: string;
}

export interface CompletionItem {
    label: string;
    kind: "variable" | "field";
    detail?: string;
    description?: string;
}

export interface LocationInfo {
    file?: string;
    line: number;
    character: number;
    end_line: number;
    end_character: number;
}

export interface ReplResult {
    output?: string;
    effects?: string[];
    error?: string;
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

    async analyze(files: Record<string, string>): Promise<DiagnosticInfo[]> {
        return (await this.request({ type: "analyze", files })) as DiagnosticInfo[];
    }

    async hover(
        files: Record<string, string>,
        file: string,
        line: number,
        col: number,
    ): Promise<HoverInfo | null> {
        return (await this.request({ type: "hover", files, file, line, col })) as HoverInfo | null;
    }

    async completions(
        files: Record<string, string>,
        file: string,
        line: number,
        col: number,
    ): Promise<CompletionItem[]> {
        return (await this.request({
            type: "completions",
            files,
            file,
            line,
            col,
        })) as CompletionItem[];
    }

    async gotoDefinition(
        files: Record<string, string>,
        file: string,
        line: number,
        col: number,
    ): Promise<LocationInfo | null> {
        return (await this.request({
            type: "gotoDefinition",
            files,
            file,
            line,
            col,
        })) as LocationInfo | null;
    }

    async format(source: string): Promise<string | null> {
        return (await this.request({ type: "format", source })) as string | null;
    }

    async replInit(): Promise<void> {
        await this.request({ type: "replInit" });
    }

    async replEval(files: Record<string, string>, line: string): Promise<ReplResult> {
        return (await this.request({ type: "replEval", files, line })) as ReplResult;
    }

    async replReset(): Promise<void> {
        await this.request({ type: "replReset" });
    }

    dispose() {
        this.worker.terminate();
    }
}
