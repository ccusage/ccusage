import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { describe, it, mock } from 'node:test';
import {
	createNativeSpawner,
	ensureNativeBinaryExecutable,
	isMainModule,
	resolveCliRuntime,
} from './cli.js';
import packageJson from '../package.json' with { type: 'json' };

function throwing(code: string, message: string) {
	return () => {
		throw Object.assign(new Error(message), { code });
	};
}

function stubChild() {
	const kill = mock.fn(() => true);
	return { child: Object.assign(new EventEmitter(), { kill }), kill };
}

void describe(resolveCliRuntime.name, () => {
	void it('resolves the native package binary for the current supported platform', () => {
		const actual = resolveCliRuntime({
			arch: 'arm64',
			argv: [],
			platform: 'darwin',
			resolvePath: (id) => {
				assert.equal(id, '@ccusage/ccusage-darwin-arm64/bin/ccusage');
				return '/native/bin/ccusage';
			},
		});

		assert.deepEqual(actual, { args: [], command: '/native/bin/ccusage' });
	});

	void it('resolves the Windows native package binary with the exe suffix', () => {
		const actual = resolveCliRuntime({
			arch: 'arm64',
			argv: [],
			platform: 'win32',
			resolvePath: (id) => {
				assert.equal(id, '@ccusage/ccusage-win32-arm64/bin/ccusage.exe');
				return 'C:\\native\\bin\\ccusage.exe';
			},
		});

		assert.deepEqual(actual, { args: [], command: 'C:\\native\\bin\\ccusage.exe' });
	});

	void it('prefers the matching native package binary when it is available', () => {
		assert.deepEqual(
			resolveCliRuntime({
				arch: 'arm64',
				argv: ['daily'],
				platform: 'darwin',
				resolvePath: () => '/app/node_modules/@ccusage/ccusage-darwin-arm64/bin/ccusage',
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
				platform: 'darwin',
				resolvePath: throwing(
					'MODULE_NOT_FOUND',
					"Cannot find module '@ccusage/ccusage-darwin-arm64'",
				),
			}),
			{
				errorMessage: `Missing optional native package "@ccusage/ccusage-darwin-arm64@${packageJson.version}" for darwin-arm64. It may be absent because of npm --before/--min-release-age, mise minimum_release_age, or --omit=optional. Reinstall ccusage with optional dependencies enabled and no release-age cutoff.\n`,
			},
		);
	});

	void it('reports an installed native package whose binary is missing', () => {
		assert.deepEqual(
			resolveCliRuntime({
				arch: 'arm64',
				argv: ['daily'],
				platform: 'darwin',
				resolvePath: (id) => {
					if (id === '@ccusage/ccusage-darwin-arm64/package.json') {
						return '/native/package.json';
					}
					return throwing('MODULE_NOT_FOUND', `Cannot find module '${id}'`)();
				},
			}),
			{
				errorMessage:
					'Native package "@ccusage/ccusage-darwin-arm64" for darwin-arm64 is installed, but its bin/ccusage is missing. Reinstall ccusage.\n',
			},
		);
	});

	void it('reports why an installed native package cannot be resolved', () => {
		assert.deepEqual(
			resolveCliRuntime({
				arch: 'arm64',
				argv: ['daily'],
				platform: 'darwin',
				resolvePath: throwing(
					'ERR_PACKAGE_PATH_NOT_EXPORTED',
					'Package subpath \'./bin/ccusage\' is not defined by "exports"',
				),
			}),
			{
				errorMessage: `Cannot resolve native package "@ccusage/ccusage-darwin-arm64" for darwin-arm64: Package subpath './bin/ccusage' is not defined by "exports"\n`,
			},
		);
	});

	void it('reports unsupported native binary targets without suggesting an optional package', () => {
		assert.deepEqual(
			resolveCliRuntime({
				arch: 'riscv64',
				argv: [],
				platform: 'freebsd',
				resolvePath: () => assert.fail('an unsupported target has no package to resolve'),
			}),
			{
				errorMessage: 'ccusage does not provide a native binary for freebsd-riscv64.\n',
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
		const { child, kill } = stubChild();
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
		const { child, kill } = stubChild();
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
		const { child, kill } = stubChild();
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
		const { child, kill } = stubChild();
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
