#!/usr/bin/env node
// @ts-check
import { spawn } from 'node:child_process';
import { chmodSync, realpathSync, statSync } from 'node:fs';
import { createRequire } from 'node:module';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
/** @type {{ optionalDependencies: Record<string, string>; version: string }} */
const packageJson = require('../package.json');
const wrapperVersion = packageJson.version;

/**
 * @typedef {{ args: string[]; command: string } | { errorMessage: string }} CliRuntime
 * @typedef {{ mode: number }} FileMode
 * @typedef {{ error?: Error; signal?: NodeJS.Signals | null; status: number | null }} NativeSpawnResult
 * @typedef {(command: string, args: string[]) => Promise<NativeSpawnResult>} NativeSpawner
 * @typedef {{ argvEntry?: string; moduleUrl: string; realpathPath?: (path: string) => string }} MainModuleOptions
 * @typedef {{ off: (signal: NodeJS.Signals, handler: () => void) => unknown; on: (signal: NodeJS.Signals, handler: () => void) => unknown }} SignalSource
 * @typedef {{ kill: (signal: NodeJS.Signals) => unknown; on: import('node:events').EventEmitter['on'] }} NativeChild
 * @typedef {(command: string, args: string[], options: { stdio: 'inherit' }) => NativeChild} NativeSpawnProcess
 */

/**
 * @param {string} [platform]
 * @param {string} [arch]
 */
function getNativePackageName(platform = process.platform, arch = process.arch) {
	const packageName = `@ccusage/ccusage-${platform}-${arch}`;
	return Object.hasOwn(packageJson.optionalDependencies, packageName) ? packageName : undefined;
}

/**
 * @param {string} [platform]
 */
function getNativeBinarySubpath(platform = process.platform) {
	return platform === 'win32' ? 'bin/ccusage.exe' : 'bin/ccusage';
}

/**
 * @param {{ arch?: string; argv: string[]; platform?: string; resolvePath?: (id: string) => string }} options
 * @returns {CliRuntime}
 */
function resolveCliRuntime({
	argv,
	arch = process.arch,
	platform = process.platform,
	resolvePath = (id) => require.resolve(id),
}) {
	const target = `${platform}-${arch}`;
	const packageName = getNativePackageName(platform, arch);
	if (packageName == null) {
		return {
			errorMessage: `ccusage does not provide a native binary for ${target}.\n`,
		};
	}

	const binarySubpath = getNativeBinarySubpath(platform);
	try {
		return { args: argv, command: resolvePath(`${packageName}/${binarySubpath}`) };
	} catch (binaryError) {
		if (!isModuleNotFound(binaryError)) {
			return { errorMessage: cannotResolveMessage(packageName, target, binaryError) };
		}
	}

	try {
		resolvePath(`${packageName}/package.json`);
	} catch (manifestError) {
		if (isModuleNotFound(manifestError)) {
			return {
				errorMessage: `Missing optional native package "${packageName}@${wrapperVersion}" for ${target}. It may be absent because of npm --before/--min-release-age, mise minimum_release_age, or --omit=optional. Reinstall ccusage with optional dependencies enabled and no release-age cutoff.\n`,
			};
		}
		return { errorMessage: cannotResolveMessage(packageName, target, manifestError) };
	}
	return {
		errorMessage: `Native package "${packageName}" for ${target} is installed, but its ${binarySubpath} is missing. Reinstall ccusage.\n`,
	};
}

/**
 * @param {string} packageName
 * @param {string} target
 * @param {unknown} error
 */
function cannotResolveMessage(packageName, target, error) {
	return `Cannot resolve native package "${packageName}" for ${target}: ${errorMessage(error)}\n`;
}

/**
 * @param {unknown} error
 */
function isModuleNotFound(error) {
	return error instanceof Error && 'code' in error && error.code === 'MODULE_NOT_FOUND';
}

/**
 * @param {unknown} error
 */
function errorMessage(error) {
	return error instanceof Error ? error.message : String(error);
}

/**
 * @param {{ binaryPath: string; chmodPath?: (path: string, mode: number) => void; platform?: string; statPath?: (path: string) => FileMode }} options
 */
function ensureNativeBinaryExecutable({
	binaryPath,
	chmodPath = chmodSync,
	platform = process.platform,
	statPath = statSync,
}) {
	if (platform === 'win32') {
		return undefined;
	}

	try {
		const mode = statPath(binaryPath).mode;
		if ((mode & 0o111) !== 0) {
			return undefined;
		}
		chmodPath(binaryPath, 0o755);
		return undefined;
	} catch (error) {
		return `ccusage native binary is not executable: ${errorMessage(error)}\n`;
	}
}

/**
 * @param {MainModuleOptions} options
 */
function isMainModule({ argvEntry = process.argv[1], moduleUrl, realpathPath = realpathSync }) {
	if (argvEntry == null) {
		return false;
	}

	const modulePath = fileURLToPath(moduleUrl);
	try {
		return realpathPath(modulePath) === realpathPath(argvEntry);
	} catch {
		return modulePath === argvEntry;
	}
}

/**
 * @param {string} [platform]
 * @returns {NodeJS.Signals[]}
 */
function getForwardedSignals(platform = process.platform) {
	if (platform === 'win32') {
		return ['SIGINT', 'SIGBREAK', 'SIGHUP'];
	}

	return ['SIGINT', 'SIGTERM', 'SIGHUP', 'SIGQUIT'];
}

/**
 * @param {{ signalSource?: SignalSource; spawnProcess?: NativeSpawnProcess; platform?: string }} [options]
 * @returns {NativeSpawner}
 */
function createNativeSpawner({
	platform = process.platform,
	signalSource = process,
	spawnProcess = spawn,
} = {}) {
	const forwardedSignals = getForwardedSignals(platform);

	return async (command, args) =>
		new Promise((resolve) => {
			const child = spawnProcess(command, args, { stdio: 'inherit' });
			const signalHandlers = new Map();
			const cleanup = () => {
				for (const [signal, handler] of signalHandlers) {
					signalSource.off(signal, handler);
				}
				signalHandlers.clear();
			};
			/**
			 * @param {NodeJS.Signals} signal
			 */
			const handleSignal = (signal) => {
				child.kill(signal);
			};

			child.on('error', (error) => {
				cleanup();
				resolve({
					error,
					signal: null,
					status: null,
				});
			});
			child.on('exit', (status, signal) => {
				cleanup();
				resolve({
					signal,
					status,
				});
			});

			for (const signal of forwardedSignals) {
				const handler = () => handleSignal(signal);
				signalSource.on(signal, handler);
				signalHandlers.set(signal, handler);
			}
		});
}

/**
 * @param {string[]} argv
 * @param {NativeSpawner} [spawnNative]
 */
async function runCli(argv, spawnNative = createNativeSpawner()) {
	const runtime = resolveCliRuntime({ argv });
	if ('errorMessage' in runtime) {
		process.stderr.write(runtime.errorMessage);
		return 1;
	}

	const executableError = ensureNativeBinaryExecutable({
		binaryPath: runtime.command,
	});
	if (executableError != null) {
		process.stderr.write(executableError);
		return 1;
	}

	const result = await spawnNative(runtime.command, runtime.args);
	if (result.error != null) {
		process.stderr.write(`${result.error.message}\n`);
		return 1;
	}
	if (result.signal != null) {
		process.kill(process.pid, result.signal);
		return 1;
	}
	return result.status ?? 1;
}

if (isMainModule({ moduleUrl: import.meta.url })) {
	process.exitCode = await runCli(process.argv.slice(2));
}

export { createNativeSpawner, ensureNativeBinaryExecutable, isMainModule, resolveCliRuntime };
