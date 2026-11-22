let ls = null;

self.onmessage = async (event) => {
    const msg = event.data;
    if (msg.kind === 'init') {
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
            ls = mod.exports.createLanguageServer();
            ls.setSdk(
                new Int8Array(
                    msg.compileSdk.buffer,
                    msg.compileSdk.byteOffset,
                    msg.compileSdk.byteLength
                )
            );
            ls.setNotificationHandler((notificationJson) => {
                postMessage({ kind: 'msg', data: notificationJson });
            });
            for (const f of msg.files) {
                ls.setWorkspaceFile(`file:///workdir/${f.name}`, f.content);
            }
            postMessage({ kind: 'ready' });
        } catch (e) {
            postMessage({ kind: 'error', message: String(e?.message || e) });
        }
        return;
    }

    if (msg.kind === 'msg') {
        if (!ls) {
            return;
        }
        try {
            let isExit = false;
            try {
                isExit = JSON.parse(msg.data)?.method === 'exit';
            } catch (_) {}
            const resp = ls.handleMessage(msg.data);
            if (resp != null) {
                postMessage({ kind: 'msg', data: resp });
            }
            if (isExit) {
                postMessage({ kind: 'exit' });
                self.close();
            }
        } catch (e) {
            postMessage({ kind: 'error', message: String(e?.message || e) });
        }
    }
};
