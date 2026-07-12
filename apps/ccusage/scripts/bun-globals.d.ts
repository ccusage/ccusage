declare namespace Bun {
	interface FileSink {
		write(data: string): Promise<number>;
		end(): Promise<void>;
	}

	interface BunFile {
		writer(): FileSink;
	}
}

declare const Bun: {
	file(path: string): Bun.BunFile;
	$(strings: TemplateStringsArray, ...values: readonly unknown[]): Promise<unknown>;
};
