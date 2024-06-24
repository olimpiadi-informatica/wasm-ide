import init, {threadRunFn} from './worker.js';

self.onmessage = async (msg) => {
  let data = msg['data'];
  if ('module' in data) {
    await init(data['module'], data['memory']);
  }
  if ('workerIndex' in data) {
    threadRunFn(data['workerIndex'], data['arg']);
  }
};
