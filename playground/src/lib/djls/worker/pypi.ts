// Installs third-party packages from PyPI into the virtual project, without Pyodide: djls reads
// Python and template source statically and never runs it, so unpacking a pure-Python wheel's
// files into site-packages is a complete install. There is no dependency resolution; the
// playground's package list names every package. pypi.org's JSON API and files.pythonhosted.org
// both send `Access-Control-Allow-Origin: *`.

/** A conventional venv under the project root, which djls's environment discovery finds. */
export const SITE_PACKAGES = '.venv/lib/python3.12/site-packages/';
export const PYVENV_CFG = {
	path: '.venv/pyvenv.cfg',
	content: 'home = /usr/bin\nversion = 3.12.0\n'
};

/** The fields of PyPI's JSON API (https://docs.pypi.org/api/json/) this module reads. */
interface PypiProject {
	info: { version: string };
	releases: Record<string, PypiFile[]>;
}

interface PypiFile {
	filename: string;
	url: string;
	packagetype: string;
	yanked: boolean;
}

export interface Wheel {
	filename: string;
	version: string;
	url: string;
}

/** The newest release with a pure-Python wheel, or the pinned one for `name==version`. */
export async function resolveWheel(requirement: string): Promise<Wheel> {
	const [name, pin] = requirement.split('==').map((part) => part.trim());
	const response = await fetch(`https://pypi.org/pypi/${encodeURIComponent(name)}/json`);
	if (response.status === 404) throw new Error(`${name} is not on PyPI`);
	if (!response.ok) throw new Error(`PyPI returned ${response.status} for ${name}`);
	const project = (await response.json()) as PypiProject;
	if (pin && !(pin in project.releases)) throw new Error(`${name} has no release ${pin}`);
	const versions = pin ? [pin] : [project.info.version, ...Object.keys(project.releases).reverse()];
	for (const version of versions) {
		const files = (project.releases[version] ?? []).filter((file) => !file.yanked);
		const wheel = files.find(
			(file) => file.packagetype === 'bdist_wheel' && file.filename.endsWith('-py3-none-any.whl')
		);
		if (wheel) return { filename: wheel.filename, version, url: wheel.url };
	}
	const all = Object.values(project.releases).flat();
	if (all.some((file) => file.packagetype === 'bdist_wheel')) {
		throw new Error(`${name} publishes no pure-Python (py3-none-any) wheel`);
	}
	throw new Error(`${name} publishes only an sdist, no wheel`);
}

async function inflateRaw(bytes: Uint8Array<ArrayBuffer>) {
	const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream('deflate-raw'));
	return new Uint8Array(await new Response(stream).arrayBuffer());
}

/**
 * A minimal zip reader: end-of-central-directory, then the central directory, then each local
 * header. Handles stored (0) and deflated (8) entries, which is all wheels use. No zip64.
 */
async function* zipEntries(buffer: Uint8Array<ArrayBuffer>) {
	const view = new DataView(buffer.buffer, buffer.byteOffset, buffer.byteLength);
	let eocd = buffer.length - 22;
	while (eocd >= 0 && view.getUint32(eocd, true) !== 0x06054b50) eocd--;
	if (eocd < 0) throw new Error('the wheel is not a zip archive');
	const count = view.getUint16(eocd + 10, true);
	let at = view.getUint32(eocd + 16, true);
	const decoder = new TextDecoder();
	for (let i = 0; i < count; i++) {
		const method = view.getUint16(at + 10, true);
		const compressedSize = view.getUint32(at + 20, true);
		const nameLength = view.getUint16(at + 28, true);
		const extraLength = view.getUint16(at + 30, true);
		const commentLength = view.getUint16(at + 32, true);
		const localHeader = view.getUint32(at + 42, true);
		const name = decoder.decode(buffer.subarray(at + 46, at + 46 + nameLength));
		at += 46 + nameLength + extraLength + commentLength;
		if (name.endsWith('/')) continue;
		const start =
			localHeader +
			30 +
			view.getUint16(localHeader + 26, true) +
			view.getUint16(localHeader + 28, true);
		const body = buffer.subarray(start, start + compressedSize);
		yield { name, read: () => (method === 0 ? Promise.resolve(body) : inflateRaw(body)) };
	}
}

// djls needs only Python modules and templates; `.txt` covers templates some packages ship with
// that suffix. Static assets, locale files, and the rest of `.dist-info` are skipped.
const WANTED = /\.(py|html|txt)$/;

/** Downloads a wheel and returns its source files as site-packages paths and text. */
export async function unpackWheel(wheel: Wheel) {
	const response = await fetch(wheel.url);
	if (!response.ok) throw new Error(`Downloading ${wheel.filename} failed: ${response.status}`);
	const bytes = new Uint8Array(await response.arrayBuffer());
	const decoder = new TextDecoder();
	const files = new Map<string, string>();
	for await (const { name, read } of zipEntries(bytes)) {
		if (WANTED.test(name)) files.set(SITE_PACKAGES + name, decoder.decode(await read()));
	}
	return files;
}
