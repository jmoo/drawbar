// drawbar's library writer, a dedicated worker. It writes files into the origin
// private file system through sync access handles, which no other context can hold.
// The page's wasm drives it; crates/drawbar/src/store/web.rs is the other side.
//
// A request is {id, op, path, ...}. `path` is "/"-joined from the private root, and
// the folders it names must exist. Requests run one at a time, in the order they
// arrive, and each gets one reply: {id, ok: true, value} or {id, ok: false, name,
// message}, where `name` is the DOMException's name.
//
//   lock     Hold `path` open for as long as the worker lives. The value is false when
//            another worker holds it, which is how a second tab knows to only read.
//   begin    Create `path`, or empty it, and hold it open for writing.
//   write    Write `data`, a transferred ArrayBuffer, at byte `at` of `path`.
//   end      Flush `path` to storage and let it go.
//   abandon  Let `path` go, if it is held, and delete it.
//   unlock   Let go of the lock and of every file held open. Stopping the worker lets
//            them go only when the browser gets to it, so the next library's lock may
//            otherwise find them still held.
//
// ⚠️ Before Chrome 108 and Safari 16.4, a handle's flush, truncate and close returned
// promises. Awaiting them works for both.

"use strict";

/** The lock's handle, held and never used. */
let lock = null;
/** Handles open for writing, by path. */
const held = new Map();

async function locate(path, create) {
  const names = path.split("/");
  const leaf = names.pop();
  let dir = await navigator.storage.getDirectory();
  for (const name of names) {
    dir = await dir.getDirectoryHandle(name);
  }
  return { dir, leaf, file: create ? await dir.getFileHandle(leaf, { create }) : null };
}

function holding(path) {
  const access = held.get(path);
  if (!access) {
    throw new DOMException(`${path} is not open for writing`, "InvalidStateError");
  }
  return access;
}

const ops = {
  async lock({ path }) {
    if (lock) {
      return true;
    }
    const { file } = await locate(path, true);
    try {
      lock = await file.createSyncAccessHandle();
      return true;
    } catch (e) {
      if (e.name === "NoModificationAllowedError") {
        return false;
      }
      throw e;
    }
  },

  async begin({ path }) {
    const { file } = await locate(path, true);
    const access = await file.createSyncAccessHandle();
    held.set(path, access);
    await access.truncate(0);
  },

  async write({ path, at, data }) {
    const wrote = holding(path).write(new Uint8Array(data), { at });
    if (wrote !== data.byteLength) {
      throw new Error(`wrote ${wrote} of ${data.byteLength} bytes`);
    }
  },

  async end({ path }) {
    const access = holding(path);
    held.delete(path);
    try {
      await access.flush();
    } finally {
      await access.close();
    }
  },

  async unlock() {
    for (const access of held.values()) {
      await access.close();
    }
    held.clear();
    if (lock) {
      await lock.close();
      lock = null;
    }
  },

  async abandon({ path }) {
    const access = held.get(path);
    held.delete(path);
    if (access) {
      await access.close();
    }
    const { dir, leaf } = await locate(path, false);
    try {
      await dir.removeEntry(leaf);
    } catch (e) {
      if (e.name !== "NotFoundError") {
        throw e;
      }
    }
  },
};

async function answer(request) {
  const id = request?.id;
  try {
    const op = ops[request?.op];
    if (!op) {
      throw new DOMException(`no operation ${request?.op}`, "NotSupportedError");
    }
    const value = await op(request);
    postMessage({ id, ok: true, value });
  } catch (e) {
    try {
      postMessage({
        id,
        ok: false,
        name: e?.name || "Error",
        message: e?.message || String(e),
      });
    } catch {
      // A request whose id cannot be cloned cannot be answered.
    }
  }
}

let queue = Promise.resolve();
onmessage = ({ data }) => {
  const run = () => answer(data);
  queue = queue.then(run, run);
};
