const encoder = new TextEncoder();
const decoder = new TextDecoder();

function isWhitespace(byte) {
    return byte === 0x20 || byte === 0x0a || byte === 0x0d || byte === 0x09;
}

self.onmessage = function (e) {
    try {
        const data = e.data;
        let input = new Uint8Array(data.input);

        function readByte() {
            if (input.length === 0) {
                return null;
            }
            const byte = input[0];
            input = input.subarray(1);
            return byte;
        }

        function skipWhitespace() {
            while (input.length > 0 && isWhitespace(input[0])) {
                input = input.subarray(1);
            }
        }

        function readString() {
            skipWhitespace();
            if (input.length === 0) {
                return null;
            }

            let end = 0;
            while (end < input.length && !isWhitespace(input[end])) {
                end += 1;
            }

            const token = decoder.decode(input.subarray(0, end));
            input = input.subarray(end);
            return token;
        }

        function readInt() {
            const token = readString();
            if (token === null) {
                return null;
            }
            return parseInt(token, 10);
        }

        function readFloat() {
            const token = readString();
            if (token === null) {
                return null;
            }
            return parseFloat(token);
        }

        function readChar() {
            const byte = readByte();
            if (byte === null) {
                return null;
            }
            return decoder.decode(Uint8Array.of(byte));
        }

        function readLine() {
            if (input.length === 0) {
                return null;
            }

            let end = 0;
            while (end < input.length && input[end] !== 0x0a && input[end] !== 0x0d) {
                end += 1;
            }

            const line = decoder.decode(input.subarray(0, end));
            input = input.subarray(end);

            if (input[0] === 0x0d) {
                input = input.subarray(1);
            }
            if (input[0] === 0x0a) {
                input = input.subarray(1);
            }

            return line;
        }

        function write(...values) {
            const text = values.map((value) => String(value)).join("");
            self.postMessage({ StdoutChunk: encoder.encode(text) });
        }

        function writeln(...values) {
            write(...values, "\n");
        }

        eval(data.code);
        self.postMessage({ Success: null });
    } catch (err) {
        self.postMessage({ Error: err.toString() });
    }
};
