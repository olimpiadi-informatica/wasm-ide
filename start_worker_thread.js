import init, {threadRunFn} from './worker.js';

let setInitialized;
let initialized = new Promise((resolve, _) => {
  setInitialized = resolve;
});

self.onmessage = async (msg) => {
  let data = msg['data'];
  if ('module' in data) {
    await init(data['module'], data['memory']);
    setInitialized();
  }
  if ('workerIndex' in data) {
    await initialized;
    threadRunFn(data['workerIndex'], data['arg']);
  }
};
