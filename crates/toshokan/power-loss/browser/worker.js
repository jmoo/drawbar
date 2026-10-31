// Starts the export `entry` of the crash page's module, handing it the first message.
import init, * as bundle from "./browser_crash.js";

self.onmessage = async ({ data }) => {
  self.onmessage = null;
  const start = Object.hasOwn(bundle, data.entry) ? bundle[data.entry] : undefined;
  if (typeof start !== "function") {
    throw new Error(`the crash page's module exports no ${data.entry}`);
  }
  await init({ module_or_path: data.module });
  await start(data);
};
