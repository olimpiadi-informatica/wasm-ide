self.onmessage = async (event) => {
    const msg = event.data;
    const ctrl = new Int32Array(msg.stdinChannel, 0, 2);
    const sharedPayload = new Uint8Array(msg.stdinChannel, 8);
    const stdinBuf = new Uint8Array(sharedPayload.length);
    let stdinPos = 0;
    let stdinLen = 0;
    let stdinEof = false;

    function refillStdin() {
        if (stdinEof) {
            return false;
        }
        Atomics.store(ctrl, 0, 0);
        postMessage({ kind: 'stdin_read' });
        Atomics.wait(ctrl, 0, 0);
        const n = Atomics.load(ctrl, 1);
        if (n <= 0) {
            stdinEof = true;
            stdinPos = 0;
            stdinLen = 0;
            return false;
        }
        stdinBuf.set(sharedPayload.subarray(0, n), 0);
        stdinPos = 0;
        stdinLen = n;
        return true;
    }

    function readStdin() {
        if (stdinPos >= stdinLen && !refillStdin()) {
            return -1;
        }
        return stdinBuf[stdinPos++];
    }

    function readStdinNonBlocking() {
        if (stdinPos < stdinLen) {
            return stdinBuf[stdinPos++];
        }
        return -1;
    }

    const encoder = new TextEncoder();
    let stdoutBuf = '';
    let stderrBuf = '';

    function flushStdout() {
        if (stdoutBuf.length === 0) {
            return;
        }
        const bytes = encoder.encode(stdoutBuf);
        stdoutBuf = '';
        postMessage({ kind: 'stdout', data: bytes }, [bytes.buffer]);
    }

    function flushStderr() {
        if (stderrBuf.length === 0) {
            return;
        }
        const bytes = encoder.encode(stderrBuf);
        stderrBuf = '';
        postMessage({ kind: 'stderr', data: bytes }, [bytes.buffer]);
    }

    function putcharStdout(ch) {
        stdoutBuf += String.fromCharCode(ch);
        if (ch === 10 || (stdoutBuf.length >= 4096 && (ch < 0xd800 || ch > 0xdbff))) {
            flushStdout();
        }
    }

    function putcharStderr(ch) {
        stderrBuf += String.fromCharCode(ch);
        if (ch === 10 || (stderrBuf.length >= 4096 && (ch < 0xd800 || ch > 0xdbff))) {
            flushStderr();
        }
    }

    let exitCode = 0;
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

        const app = await runtime.load(msg.appWasm, {
            onExit(code) {
                throw new runtime.ExitError(code);
            },
            installImports(imports) {
                imports.teavmConsole.putcharStdout = putcharStdout;
                imports.teavmConsole.putcharStderr = putcharStderr;
                imports.teavmConsole.readStdin = readStdin;
                imports.teavmConsole.readStdinNonBlocking = readStdinNonBlocking;
            },
        });
        app.exports.main([]);
    } catch (e) {
        if (e?.name === 'TeaVMExitError') {
            exitCode = typeof e.exitCode === 'number' ? e.exitCode : 0;
        } else {
            exitCode = 1;
            const msgText =
                e?.message && e.message !== '(could not fetch message)'
                    ? e.message
                    : String(e);
            stderrBuf += `Exception in thread "main": ${msgText}\n`;
        }
    } finally {
        flushStdout();
        flushStderr();
        postMessage({ kind: 'exit', code: exitCode });
        self.close();
    }
};
