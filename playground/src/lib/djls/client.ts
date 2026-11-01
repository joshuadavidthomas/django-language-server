import type { Method, Methods, Response, Result } from '#lib/djls/worker/protocol.js';
import DjlsWorker from '#lib/djls/worker/worker.ts?worker';

interface Pending {
	resolve(result: Result): void;
	reject(error: Error): void;
}

export class WorkerCrashed extends Error {}

/** Talks to djls in a Web Worker. Requests are answered in the order they are sent. */
export class DjlsClient {
	#worker!: Worker;
	#nextId = 0;
	#pending = new Map<number, Pending>();
	#onCrash: (error: string) => void;

	constructor(onCrash: (error: string) => void) {
		this.#onCrash = onCrash;
		this.#spawn();
	}

	restart() {
		this.dispose();
		this.#spawn();
	}

	dispose() {
		this.#worker.terminate();
		for (const { reject } of this.#pending.values()) reject(new WorkerCrashed('worker stopped'));
		this.#pending.clear();
	}

	call<M extends Method>(method: M, params: Methods[M]['params']): Promise<Methods[M]['result']> {
		const id = this.#nextId++;
		return new Promise((resolve, reject) => {
			this.#pending.set(id, { resolve: resolve as (result: Result) => void, reject });
			this.#worker.postMessage({ id, method, params });
		});
	}

	#spawn() {
		this.#worker = new DjlsWorker();
		this.#worker.onmessage = (event: MessageEvent<Response>) => {
			const response = event.data;
			const pending = this.#pending.get(response.id);
			this.#pending.delete(response.id);
			if (response.ok) {
				pending?.resolve(response.result);
			} else if (response.fatal) {
				pending?.reject(new WorkerCrashed(response.error));
				this.#onCrash(response.error);
			} else {
				pending?.reject(new Error(response.error));
			}
		};
		// Loading the worker or its wasm failed; nothing queued will ever be answered.
		this.#worker.onerror = (event) => {
			for (const { reject } of this.#pending.values()) {
				reject(new Error(event.message || 'djls worker failed to load'));
			}
			this.#pending.clear();
		};
	}
}
