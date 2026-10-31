// Starts the export `entry` of the crash page's module, handing it the first message.
import init, * as bundle from "./browser_crash.js";

self.onmessage = async ({ data }) => {
  self.onmessage = null;
  await init({ module_or_path: data.module });
  await bundle[data.entry](data);
};
