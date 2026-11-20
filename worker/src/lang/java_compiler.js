function resolveMainClass(primaryClass, mains) {
    if (primaryClass && mains.includes(primaryClass)) {
        return primaryClass;
    }
    const namedMain = mains.find(
        (name) => name === 'Main' || name.endsWith('.Main')
    );
    if (namedMain) {
        return namedMain;
    }
    return mains[0];
}

self.onmessage = async (event) => {
    const msg = event.data;
    try {
        const runtimeUrl = URL.createObjectURL(
            new Blob([msg.runtimeJs], { type: 'text/javascript' })
        );
        let runtime;
        try {
            runtime = await import(/* @vite-ignore */ runtimeUrl);
        } finally {
            URL.revokeObjectURL(runtimeUrl);
        }

        const mod = await runtime.load(msg.compilerWasm, {});
        const compiler = mod.exports.createCompiler();
        compiler.setSdk(
            new Int8Array(
                msg.compileSdk.buffer,
                msg.compileSdk.byteOffset,
                msg.compileSdk.byteLength
            )
        );
        compiler.setTeaVMClasslib(
            new Int8Array(
                msg.runtimeSdk.buffer,
                msg.runtimeSdk.byteOffset,
                msg.runtimeSdk.byteLength
            )
        );
        compiler.onDiagnostic((d) => {
            const loc = d.fileName
                ? `${d.fileName}:${d.lineNumber}:${d.columnNumber}: `
                : '';
            postMessage({
                kind: 'diag',
                text: `${loc}${d.severity}: ${d.message}\n`,
            });
        });

        for (const f of msg.files) {
            compiler.addSourceFile(f.name, f.content);
        }
        if (!compiler.compile()) {
            postMessage({ kind: 'failed', message: 'Compilation failed' });
            return;
        }

        const mains = Array.from(compiler.detectMainClasses() || []);
        if (mains.length === 0) {
            postMessage({
                kind: 'failed',
                message: 'No class with a main(String[]) method found',
            });
            return;
        }
        const mainClass = resolveMainClass(msg.primaryClass, mains);
        const genOk = await compiler.generateWebAssembly({
            mainClass,
            outputName: 'app',
            obfuscated: false,
            asyncEnabled: false,
        });
        if (!genOk) {
            postMessage({
                kind: 'failed',
                message: 'WebAssembly code generation failed',
            });
            return;
        }

        const out = compiler.getWebAssemblyOutputFile('app.wasm');
        if (!out) {
            postMessage({
                kind: 'failed',
                message: 'Compiler produced no WebAssembly output',
            });
            return;
        }
        const wasmBytes = new Uint8Array(
            out.buffer,
            out.byteOffset,
            out.byteLength
        ).slice();
        postMessage({ kind: 'ok', wasm: wasmBytes }, [wasmBytes.buffer]);
    } catch (e) {
        postMessage({ kind: 'error', message: String(e?.message || e) });
    } finally {
        self.close();
    }
};
