import type {
    StatusResponse,
    ToolSpec,
    CronJob,
    Integration,
    DiagResult,
    MemoryEntry,
    CostSummary,
    CliTool,
    HealthSnapshot,
} from "../types/api";
import { clearToken, getToken, setToken } from "./auth";

// ---------------------------------------------------------------------------
// Base fetch wrapper
// ---------------------------------------------------------------------------

export class UnauthorizedError extends Error {
    constructor() {
        super("Unauthorized");
        this.name = "UnauthorizedError";
    }
}

export async function apiFetch<T = unknown>(
    path: string,
    options: RequestInit = {},
): Promise<T> {
    const token = getToken();
    const headers = new Headers(options.headers);

    if (token) {
        headers.set("Authorization", `Bearer ${token}`);
    }

    if (
        options.body &&
        typeof options.body === "string" &&
        !headers.has("Content-Type")
    ) {
        headers.set("Content-Type", "application/json");
    }

    const response = await fetch(path, { ...options, headers });

    if (response.status === 401) {
        clearToken();
        window.dispatchEvent(new Event("zeroclaw-unauthorized"));
        throw new UnauthorizedError();
    }

    if (!response.ok) {
        const text = await response.text().catch(() => "");
        throw new Error(
            `API ${response.status}: ${text || response.statusText}`,
        );
    }

    // Some endpoints may return 204 No Content
    if (response.status === 204) {
        return undefined as unknown as T;
    }

    return response.json() as Promise<T>;
}

function unwrapField<T>(value: T | Record<string, T>, key: string): T;
function unwrapField<T>(
    value: T[] | Record<string, T[]>,
    key: string,
    expectArray: true,
): T[];
function unwrapField<T>(
    value: unknown,
    key: string,
    expectArray?: boolean,
): T | T[] {
    if (
        value !== null &&
        typeof value === "object" &&
        !Array.isArray(value) &&
        key in (value as Record<string, unknown>)
    ) {
        const unwrapped = (value as Record<string, unknown>)[key];
        if (unwrapped !== undefined) {
            if (expectArray) {
                return Array.isArray(unwrapped) ? (unwrapped as T[]) : [];
            }
            return unwrapped as T;
        }
    }
    if (expectArray) {
        return Array.isArray(value) ? (value as T[]) : [];
    }
    return value as T;
}

// ---------------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------------

export async function pair(code: string): Promise<{ token: string }> {
    const response = await fetch("/pair", {
        method: "POST",
        headers: { "X-Pairing-Code": code },
    });

    if (!response.ok) {
        const text = await response.text().catch(() => "");
        throw new Error(
            `Pairing failed (${response.status}): ${text || response.statusText}`,
        );
    }

    const data: unknown = await response.json();
    if (
        !data ||
        typeof data !== "object" ||
        typeof (data as { token?: unknown }).token !== "string" ||
        !(data as { token: string }).token
    ) {
        throw new Error("Invalid pairing response: missing token");
    }
    const token = (data as { token: string }).token;
    setToken(token);
    return { token };
}

// ---------------------------------------------------------------------------
// Public health (no auth required)
// ---------------------------------------------------------------------------

export async function getPublicHealth(): Promise<{
    require_pairing: boolean;
    paired: boolean;
}> {
    const response = await fetch("/health");
    if (!response.ok) {
        throw new Error(`Health check failed (${response.status})`);
    }
    return response.json() as Promise<{
        require_pairing: boolean;
        paired: boolean;
    }>;
}

// ---------------------------------------------------------------------------
// Status / Health
// ---------------------------------------------------------------------------

export function getStatus(): Promise<StatusResponse> {
    return apiFetch<StatusResponse>("/api/status");
}

export function getHealth(): Promise<HealthSnapshot> {
    return apiFetch<HealthSnapshot | { health: HealthSnapshot }>(
        "/api/health",
    ).then((data) => unwrapField(data, "health"));
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

export function getConfig(): Promise<string> {
    return apiFetch<string | { format?: string; content: string }>(
        "/api/config",
    ).then((data) => (typeof data === "string" ? data : data.content));
}

export function putConfig(toml: string): Promise<void> {
    return apiFetch<void>("/api/config", {
        method: "PUT",
        headers: { "Content-Type": "application/toml" },
        body: toml,
    });
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

export function getTools(): Promise<ToolSpec[]> {
    return apiFetch<ToolSpec[] | { tools: ToolSpec[] }>("/api/tools").then(
        (data) => unwrapField(data, "tools", true),
    );
}

// ---------------------------------------------------------------------------
// Cron
// ---------------------------------------------------------------------------

export function getCronJobs(): Promise<CronJob[]> {
    return apiFetch<CronJob[] | { jobs: CronJob[] }>("/api/cron").then((data) =>
        unwrapField(data, "jobs", true),
    );
}

export function addCronJob(body: {
    name?: string;
    command: string;
    schedule: string;
    enabled?: boolean;
}): Promise<CronJob> {
    return apiFetch<unknown>("/api/cron", {
        method: "POST",
        body: JSON.stringify(body),
    }).then((data) => {
        if (data && typeof data === "object" && "job" in data) {
            const job = (data as { job?: unknown }).job;
            if (job && typeof job === "object" && !Array.isArray(job)) {
                return job as CronJob;
            }
            throw new Error(
                "Invalid cron job response: missing or malformed job",
            );
        }
        // Assume direct CronJob shape (server returned the object directly)
        if (data && typeof data === "object" && "id" in data) {
            return data as CronJob;
        }
        throw new Error("Invalid cron job response");
    });
}

export function deleteCronJob(id: string): Promise<void> {
    return apiFetch<void>(`/api/cron/${encodeURIComponent(id)}`, {
        method: "DELETE",
    });
}

// ---------------------------------------------------------------------------
// Integrations
// ---------------------------------------------------------------------------

export function getIntegrations(): Promise<Integration[]> {
    return apiFetch<Integration[] | { integrations: Integration[] }>(
        "/api/integrations",
    ).then((data) => unwrapField(data, "integrations", true));
}

// ---------------------------------------------------------------------------
// Doctor / Diagnostics
// ---------------------------------------------------------------------------

export function runDoctor(): Promise<DiagResult[]> {
    return apiFetch<unknown>("/api/doctor", {
        method: "POST",
        body: JSON.stringify({}),
    }).then((data) =>
        unwrapField(
            data as DiagResult[] | Record<string, DiagResult[]>,
            "results",
            true,
        ),
    );
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

export function getMemory(
    query?: string,
    category?: string,
): Promise<MemoryEntry[]> {
    const params = new URLSearchParams();
    if (query) params.set("query", query);
    if (category) params.set("category", category);
    const qs = params.toString();
    return apiFetch<MemoryEntry[] | { entries: MemoryEntry[] }>(
        `/api/memory${qs ? `?${qs}` : ""}`,
    ).then((data) => unwrapField(data, "entries", true));
}

export function storeMemory(
    key: string,
    content: string,
    category?: string,
): Promise<void> {
    return apiFetch<unknown>("/api/memory", {
        method: "POST",
        body: JSON.stringify({ key, content, category }),
    }).then(() => undefined);
}

export function deleteMemory(key: string): Promise<void> {
    return apiFetch<void>(`/api/memory/${encodeURIComponent(key)}`, {
        method: "DELETE",
    });
}

// ---------------------------------------------------------------------------
// Cost
// ---------------------------------------------------------------------------

export function getCost(): Promise<CostSummary> {
    return apiFetch<CostSummary | { cost: CostSummary }>("/api/cost").then(
        (data) => unwrapField(data, "cost"),
    );
}

// ---------------------------------------------------------------------------
// CLI Tools
// ---------------------------------------------------------------------------

export function getCliTools(): Promise<CliTool[]> {
    return apiFetch<CliTool[] | { cli_tools: CliTool[] }>(
        "/api/cli-tools",
    ).then((data) => unwrapField(data, "cli_tools", true));
}
