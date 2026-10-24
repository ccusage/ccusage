import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { readFileSync } from 'node:fs';
import { describe, it, mock } from 'node:test';
import {
	createNativeSpawner,
	ensureNativeBinaryExecutable,
	isMainModule,
	resolveCliRuntime,
	resolveNativeBinary,
} from './cli.js';

const packageJson = JSON.parse(
	readFileSync(new URL('../package.json', import.meta.url), 'utf8'),
) as { version: string };

void describe(resolveCliRuntime.name, () => {
	void it('resolves the native package binary for the current supported platform', () => {
		const actual = resolveNativeBinary({
			arch: 'arm64',
			platform: 'darwin',
			resolvePath: (id) => {
				assert.equal(id, '@ccusage/ccusage-darwin-arm64/bin/ccusage');
				return '/native/bin/ccusage';
			},
		});

		assert.equal(actual, '/native/bin/ccusage');
	});

	void it('resolves the Windows native package binary with the exe suffix', () => {
		const actual = resolveNativeBinary({
			arch: 'arm64',
			platform: 'win32',
			resolvePath: (id) => {
				assert.equal(id, '@ccusage/ccusage-win32-arm64/bin/ccusage.exe');
				return 'C:\\native\\bin\\ccusage.exe';
			},
		});

		assert.equal(actual, 'C:\\native\\bin\\ccusage.exe');
	});

	void it('prefers the matching native package binary when it is available', () => {
		assert.deepEqual(
			resolveCliRuntime({
				argv: ['daily'],
				nativeBinaryPath: '/app/node_modules/@ccusage/ccusage-darwin-arm64/bin/ccusage',
			}),
			{
				args: ['daily'],
				command: '/app/node_modules/@ccusage/ccusage-darwin-arm64/bin/ccusage',
			},
		);
	});

	void it('fails when the native package binary is unavailable', () => {
		assert.deepEqual(
			resolveCliRuntime({
				arch: 'arm64',
				argv: ['daily'],
				nativeBinaryPath: null,
				platform: 'darwin',
				version: '20.0.38',
			}),
			{
				errorMessage:
					'ccusage native binary is not available for darwin-arm64: @ccusage/ccusage-darwin-arm64@20.0.38 is not installed.\n' +
					'The optional native dependency was likely skipped by an install-age cutoff (npm --before or --min-release-age, mise minimum_release_age) or by --omit=optional.\n' +
					'Reinstall ccusage@20.0.38 with optional dependencies once @ccusage/ccusage-darwin-arm64@20.0.38 is within the cutoff.\n',
			},
		);
	});

	void it('names the version from the package manifest when the native package is missing', () => {
		const runtime = resolveCliRuntime({
			arch: 'x64',
			argv: [],
			nativeBinaryPath: null,
			platform: 'linux',
		});

		assert.ok('errorMessage' in runtime);
		assert.match(
			runtime.errorMessage,
			new RegExp(`@ccusage/ccusage-linux-x64@${packageJson.version.replaceAll('.', '\\.')} `),
		);
	});

	void it('reports unsupported platforms without a native package', () => {
		assert.deepEqual(
			resolveCliRuntime({
				arch: 'x64',
				argv: [],
				nativeBinaryPath: null,
				platform: 'freebsd',
			}),
			{
				errorMessage:
					'ccusage native binary is not available for freebsd-x64: this platform is not supported.\n',
			},
		);
	});

	void it('repairs a native binary that was extracted without executable bits', () => {
		const chmodPath = mock.fn();

		assert.equal(
			ensureNativeBinaryExecutable({
				binaryPath: '/native/bin/ccusage',
				chmodPath,
				platform: 'linux',
				statPath: () => ({ mode: 0o644 }),
			}),
			undefined,
		);
		assert.deepEqual(
			chmodPath.mock.calls.map((call) => call.arguments),
			[['/native/bin/ccusage', 0o755]],
		);
	});

	void it('does not chmod an already executable native binary', () => {
		const chmodPath = mock.fn();

		assert.equal(
			ensureNativeBinaryExecutable({
				binaryPath: '/native/bin/ccusage',
				chmodPath,
				platform: 'darwin',
				statPath: () => ({ mode: 0o755 }),
			}),
			undefined,
		);
		assert.equal(chmodPath.mock.callCount(), 0);
	});

	void it('does not chmod Windows native binaries', () => {
		const chmodPath = mock.fn();

		assert.equal(
			ensureNativeBinaryExecutable({
				binaryPath: 'C:\\native\\bin\\ccusage.exe',
				chmodPath,
				platform: 'win32',
				statPath: () => ({ mode: 0o644 }),
			}),
			undefined,
		);
		assert.equal(chmodPath.mock.callCount(), 0);
	});

	void it('treats package bin symlinks as the main module entry point', () => {
		const actual = isMainModule({
			argvEntry: '/project/node_modules/.bin/ccusage',
			moduleUrl: 'file:///project/node_modules/ccusage/src/cli.js',
			realpathPath: (path) =>
				path === '/project/node_modules/.bin/ccusage'
					? '/project/node_modules/ccusage/src/cli.js'
					: path,
		});

		assert.equal(actual, true);
	});
});

void describe(createNativeSpawner.name, () => {
	void it('forwards every supported launcher signal delivery', async () => {
		const signalSource = new EventEmitter();
		const kill = mock.fn(() => true);
		const child = /** @type {import('node:child_process').ChildProcess} */ (
			/** @type {unknown} */ (Object.assign(new EventEmitter(), { kill }))
		);
		const spawnProcess = mock.fn(() => child);
		const spawnNative = createNativeSpawner({
			platform: 'linux',
			signalSource,
			spawnProcess,
		});

		const resultPromise = spawnNative('/native/bin/ccusage', ['statusline']);
		for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP', 'SIGQUIT']) {
			signalSource.emit(signal);
			signalSource.emit(signal);
		}
		child.emit('exit', 0, null);

		assert.deepEqual(await resultPromise, { signal: null, status: 0 });
		assert.deepEqual(
			kill.mock.calls.map((call) => call.arguments),
			[
				['SIGINT'],
				['SIGINT'],
				['SIGTERM'],
				['SIGTERM'],
				['SIGHUP'],
				['SIGHUP'],
				['SIGQUIT'],
				['SIGQUIT'],
			],
		);
		const spawnCall = spawnProcess.mock.calls[0];
		assert.ok(spawnCall);
		assert.deepEqual(spawnCall.arguments, [
			'/native/bin/ccusage',
			['statusline'],
			{ stdio: 'inherit' },
		]);
	});

	void it('forwards the supported Windows console signals', async () => {
		const signalSource = new EventEmitter();
		const kill = mock.fn(() => true);
		const child = /** @type {import('node:child_process').ChildProcess} */ (
			/** @type {unknown} */ (Object.assign(new EventEmitter(), { kill }))
		);
		const spawnNative = createNativeSpawner({
			platform: 'win32',
			signalSource,
			spawnProcess: () => child,
		});

		const resultPromise = spawnNative('/native/bin/ccusage.exe', []);
		assert.deepEqual(
			['SIGINT', 'SIGBREAK', 'SIGHUP'].map((signal) => signalSource.listenerCount(signal)),
			[1, 1, 1],
		);
		signalSource.emit('SIGINT');
		signalSource.emit('SIGBREAK');
		signalSource.emit('SIGHUP');
		signalSource.emit('SIGTERM');
		assert.equal(signalSource.listenerCount('SIGTERM'), 0);
		assert.deepEqual(
			kill.mock.calls.map((call) => call.arguments),
			[['SIGINT'], ['SIGBREAK'], ['SIGHUP']],
		);
		child.emit('exit', 0, null);

		assert.deepEqual(await resultPromise, { signal: null, status: 0 });
		assert.deepEqual(
			['SIGINT', 'SIGBREAK', 'SIGHUP'].map((signal) => signalSource.listenerCount(signal)),
			[0, 0, 0],
		);
	});

	void it('removes signal listeners after the child exits', async () => {
		const signalSource = new EventEmitter();
		const kill = mock.fn(() => true);
		const child = /** @type {import('node:child_process').ChildProcess} */ (
			/** @type {unknown} */ (Object.assign(new EventEmitter(), { kill }))
		);
		const spawnNative = createNativeSpawner({
			platform: 'linux',
			signalSource,
			spawnProcess: () => child,
		});

		const resultPromise = spawnNative('/native/bin/ccusage', []);
		child.emit('exit', 7, 'SIGTERM');

		assert.deepEqual(await resultPromise, { signal: 'SIGTERM', status: 7 });
		assert.deepEqual(
			['SIGINT', 'SIGTERM', 'SIGHUP', 'SIGQUIT'].map((signal) =>
				signalSource.listenerCount(signal),
			),
			[0, 0, 0, 0],
		);
		signalSource.emit('SIGTERM');
		assert.equal(kill.mock.callCount(), 0);
	});

	void it('removes signal listeners after a child error', async () => {
		const signalSource = new EventEmitter();
		const kill = mock.fn(() => true);
		const child = /** @type {import('node:child_process').ChildProcess} */ (
			/** @type {unknown} */ (Object.assign(new EventEmitter(), { kill }))
		);
		const spawnNative = createNativeSpawner({
			platform: 'linux',
			signalSource,
			spawnProcess: () => child,
		});
		const error = new Error('spawn failed');

		const resultPromise = spawnNative('/native/bin/ccusage', []);
		child.emit('error', error);

		assert.deepEqual(await resultPromise, {
			error,
			signal: null,
			status: null,
		});
		assert.deepEqual(
			['SIGINT', 'SIGTERM', 'SIGHUP', 'SIGQUIT'].map((signal) =>
				signalSource.listenerCount(signal),
			),
			[0, 0, 0, 0],
		);
		signalSource.emit('SIGINT');
		assert.equal(kill.mock.callCount(), 0);
	});
});
