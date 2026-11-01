import type { DjlsSettings, Files, Method, Methods, Response } from "./worker/protocol";
import DjlsWorker from "./worker/worker?worker";

interface Pending {
  resolve(result: unknown): void;
  reject(error: Error): void;
}

export class WorkerCrashed extends Error {}

/** Talks to djls in a Web Worker. Requests are answered in the order they are sent. */
export class DjlsClient {
  private worker!: Worker;
  private nextId = 0;
  private pending = new Map<number, Pending>();

  constructor(private onCrash: (error: string) => void) {
    this.spawn();
  }

  /** Opens the project in the current worker; after `restart`, call it again. */
  open(files: Files, settings: DjlsSettings) {
    return this.call("open", { files, settings });
  }

  restart() {
    this.worker.terminate();
    for (const { reject } of this.pending.values()) reject(new WorkerCrashed("worker restarted"));
    this.pending.clear();
    this.spawn();
  }

  call<M extends Method>(method: M, params: Methods[M]["params"]): Promise<Methods[M]["result"]> {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (result: unknown) => void, reject });
      this.worker.postMessage({ id, method, params });
    });
  }

  private spawn() {
    this.worker = new DjlsWorker();
    this.worker.onmessage = (event: MessageEvent<Response>) => {
      const response = event.data;
      const pending = this.pending.get(response.id);
      this.pending.delete(response.id);
      if (response.ok) {
        pending?.resolve(response.result);
      } else if (response.fatal) {
        pending?.reject(new WorkerCrashed(response.error));
        this.onCrash(response.error);
      } else {
        pending?.reject(new Error(response.error));
      }
    };
    // Loading the worker or its wasm failed; nothing queued will ever be answered.
    this.worker.onerror = (event) => {
      for (const { reject } of this.pending.values()) reject(new Error(event.message || "djls worker failed to load"));
      this.pending.clear();
    };
  }
}
