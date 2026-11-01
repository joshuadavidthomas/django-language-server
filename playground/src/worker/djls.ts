// The only module that touches the generated wasm-bindgen package. If how the
// wasm or its bundled Django sources get fetched changes, change it here.
import init, { Workspace } from "../../djls-wasm/djls_wasm.js";

export type { Workspace };

export async function createWorkspace(settings: unknown): Promise<Workspace> {
  await init();
  return new Workspace(JSON.stringify(settings));
}
