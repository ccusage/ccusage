declare const Bun: {
	write(destination: string, data: string | Uint8Array): Promise<number>;
	zstdCompressSync(data: string | Uint8Array): Uint8Array;
	$(strings: TemplateStringsArray, ...values: readonly unknown[]): Promise<unknown>;
};
