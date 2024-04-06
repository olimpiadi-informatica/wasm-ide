import init, {threadRunFn} from './worker.js';

self.onmessage = async (msg) => {
  let data = msg['data'];
  let wasm = await init(data['module'], data['memory']);
  threadRunFn(data['closure'], data['arg']);
  wasm.__wbindgen_thread_destroy();
};
