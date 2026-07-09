import { connect, type Socket } from "node:net";

export class ApiClient {
	private socket: Socket | null = null;
	private nextId = 1;
	private pending = new Map<
		number,
		{ resolve: (v: unknown) => void; reject: (e: Error) => void }
	>();
	private onState: ((state: unknown) => void) | null = null;
	private closeCb: (() => void) | null = null;

	onClose(cb: () => void): void {
		this.closeCb = cb;
	}

	connect(sockPath: string): Promise<void> {
		return new Promise((resolve, reject) => {
			const socket = connect(sockPath);
			this.socket = socket;
			let buffer = "";
			socket.once("connect", () => resolve());
			socket.once("error", (err) => reject(err));
			socket.on("close", () => this.closeCb?.());
			socket.on("data", (chunk) => {
				buffer += chunk.toString();
				const lines = buffer.split("\n");
				buffer = lines.pop() ?? "";
				for (const line of lines) {
					if (!line.trim()) continue;
					this.handleFrame(line);
				}
			});
		});
	}

	private handleFrame(line: string): void {
		let frame: Record<string, unknown>;
		try {
			frame = JSON.parse(line);
		} catch {
			return;
		}
		if (frame.event === "state") {
			this.onState?.(frame.data);
			return;
		}
		const id = frame.id as number;
		const pending = this.pending.get(id);
		if (!pending) return;
		this.pending.delete(id);
		if (frame.error !== undefined) {
			pending.reject(new Error(String(frame.error)));
		} else {
			pending.resolve(frame.result);
		}
	}

	call(
		method: string,
		params?: Record<string, unknown>,
		// Most calls are quick queue/store mutations; long-running operations
		// (e.g. createWorktree, whose post-create hooks may install and build)
		// pass their own budget.
		timeoutMs = 5000,
	): Promise<unknown> {
		const socket = this.socket;
		if (!socket) return Promise.reject(new Error("not connected"));
		const id = this.nextId++;
		return new Promise((resolve, reject) => {
			const timer = setTimeout(() => {
				this.pending.delete(id);
				reject(new Error(`call timed out: ${method}`));
			}, timeoutMs);
			this.pending.set(id, {
				resolve: (v) => {
					clearTimeout(timer);
					resolve(v);
				},
				reject: (e) => {
					clearTimeout(timer);
					reject(e);
				},
			});
			socket.write(`${JSON.stringify({ id, method, params })}\n`);
		});
	}

	async subscribe(onState: (state: unknown) => void): Promise<void> {
		this.onState = onState;
		await this.call("subscribe");
	}

	close(): void {
		this.socket?.destroy();
		this.socket = null;
	}
}
