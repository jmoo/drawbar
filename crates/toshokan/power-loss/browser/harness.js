// The page the crash sweep's driver steers: it starts one worker at a time on an
// export of the crash module and queues what the worker says.
import init, { crashWorkloads } from "./browser_crash.js";

const module = await WebAssembly.compileStreaming(fetch("./browser_crash_bg.wasm"));
await init({ module_or_path: module });
const script = new URL("./worker.js", import.meta.url).href;

let worker = null;
let said = { queue: [], marks: [], lost: 0, waiting: null };

function hear(data) {
  if ("mark" in data) {
    said.marks.push(data.mark);
  } else if ("lost" in data) {
    said.lost = data.lost;
  } else {
    said.queue.push(data);
    said.waiting?.();
  }
}

async function root(path) {
  let dir = await navigator.storage.getDirectory();
  for (const name of path) {
    dir = await dir.getDirectoryHandle(name, { create: true });
  }
  return dir;
}

window.harness = {
  workloads: () => crashWorkloads(),

  // Starts a worker on `entry`, handing it `fields`.
  start(entry, fields) {
    worker?.terminate();
    said = { queue: [], marks: [], lost: 0, waiting: null };
    worker = new Worker(script, { type: "module" });
    worker.onmessage = ({ data }) => hear(data);
    worker.onerror = (event) => hear({ failed: `the worker failed: ${event.message}` });
    worker.postMessage({ entry, module, script, ...fields });
  },

  // The next thing the worker says but a mark or a lost local root, with the marks
  // and the local roots lost so far, or `failed` after `ms`.
  async next(ms) {
    const timer = new Promise((done) => setTimeout(done, ms));
    while (said.queue.length === 0) {
      const heard = new Promise((done) => (said.waiting = done));
      if ((await Promise.race([heard.then(() => true), timer.then(() => false)])) === false) {
        return { failed: `nothing was said in ${ms} ms`, marks: said.marks, lost: said.lost };
      }
    }
    return { ...said.queue.shift(), marks: said.marks, lost: said.lost };
  },

  terminate() {
    worker?.terminate();
    worker = null;
  },

  // Lets the quota run, waiting in `room`, write again.
  room: null,
  roomMade() {
    Atomics.store(this.room, 0, 1);
    Atomics.notify(this.room, 0);
  },
  quota(fields) {
    const shared = new SharedArrayBuffer(4);
    this.room = new Int32Array(shared);
    this.start("crashQuota", { ...fields, room: shared });
  },

  // Waits until no lock of the run `run` is held, and returns how long that took,
  // or -1 after `ms`.
  async locksFree(run, ms) {
    const began = performance.now();
    while (performance.now() - began < ms) {
      const { held } = await navigator.locks.query();
      if (!held.some((lock) => lock.name.includes(run))) {
        return Math.round(performance.now() - began);
      }
      await new Promise((done) => setTimeout(done, 20));
    }
    return -1;
  },

  async estimate() {
    const { usage, quota } = await navigator.storage.estimate();
    return { usage, quota };
  },

  // Fills `bytes` of the origin's quota with a file outside every run, or empties it.
  async ballast(bytes) {
    const root = await navigator.storage.getDirectory();
    await root.removeEntry("ballast").catch(() => {});
    if (bytes > 0) {
      const stream = await (await root.getFileHandle("ballast", { create: true })).createWritable();
      const chunk = new Uint8Array(1 << 20);
      for (let left = bytes; left > 0; left -= chunk.length) {
        await stream.write(chunk.subarray(0, Math.min(left, chunk.length)));
      }
      await stream.close();
    }
  },

  // Removes the run directory `name` of `parent`.
  async wipe(parent, name) {
    const dir = await root(parent);
    await dir.removeEntry(name, { recursive: true }).catch(() => {});
  },
};
document.title = "ready";
